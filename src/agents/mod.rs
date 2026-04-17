//! Runtime workers for graph-resident `agent` modules.
//!
//! Agent modules live in the audio graph so inventions can trigger them like
//! any other module, but all expensive work happens here on non-audio threads.
//! The audio module only increments trigger/reset counters; these workers
//! observe those counters, build bounded context packets from runtime snapshots,
//! call configured backends, and optionally write validated results back through
//! normal graph controls.

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::collections::HashMap;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::sync::mpsc::{self, RecvTimeoutError, Sender};
    use std::sync::Mutex;
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use serde_json::{json, Value};

    use crate::invention::{RuntimeController, RuntimeModuleInfo};
    use crate::ControlValue;

    enum HostCommand {
        Stop,
    }

    struct AgentHost {
        command_tx: Sender<HostCommand>,
        join: JoinHandle<()>,
    }

    /// Manages one background worker per running `agent` module.
    ///
    /// This mirrors the script host lifecycle used by `code` modules. Workers
    /// are started when an invention starts or an `agent` module is added at
    /// runtime, and stopped when their module or invention is removed. Keeping
    /// this manager outside the audio graph avoids network calls, process
    /// spawning, JSON parsing, and history allocation on the audio thread.
    #[derive(Default)]
    pub struct AgentManager {
        hosts: Mutex<HashMap<String, AgentHost>>,
    }

    impl AgentManager {
        /// Starts workers for every `agent` module in the current runtime.
        pub fn start_all(&self, controller: RuntimeController) {
            for module in controller.snapshot.list_modules() {
                if module.module_type == "agent" {
                    self.start_module(controller.clone(), module);
                }
            }
        }

        /// Starts or restarts the worker for a single `agent` module.
        ///
        /// The module's original config is kept as the static orchestration
        /// spec. Mutable controls such as `prompt`, `backend`, and `enabled`
        /// are read from the runtime control surface each time a request is
        /// serviced.
        pub fn start_module(&self, controller: RuntimeController, module: RuntimeModuleInfo) {
            if module.module_type != "agent" {
                return;
            }

            self.stop_module(&module.id);
            let module_id = module.id.clone();
            let (command_tx, command_rx) = mpsc::channel();
            let join =
                thread::spawn(move || run_host(module_id, module.config, controller, command_rx));
            self.hosts
                .lock()
                .unwrap()
                .insert(module.id, AgentHost { command_tx, join });
        }

        /// Stops the worker associated with `module_id`, if one is running.
        pub fn stop_module(&self, module_id: &str) {
            if let Some(host) = self.hosts.lock().unwrap().remove(module_id) {
                let _ = host.command_tx.send(HostCommand::Stop);
                let _ = host.join.join();
            }
        }

        /// Stops all running agent workers.
        pub fn stop_all(&self) {
            let module_ids: Vec<String> = self.hosts.lock().unwrap().keys().cloned().collect();
            for module_id in module_ids {
                self.stop_module(&module_id);
            }
        }
    }

    #[derive(Clone, Debug)]
    struct HistoryLimits {
        max_turns: usize,
        max_chars: usize,
    }

    #[derive(Clone, Debug)]
    struct BackendResult {
        text: String,
        backend: String,
        model: Option<String>,
    }

    #[derive(Clone, Copy, Debug)]
    struct LocalHarness {
        name: &'static str,
        command: &'static str,
        build_args: fn(&Value) -> Vec<String>,
    }

    fn run_host(
        module_id: String,
        config: Value,
        controller: RuntimeController,
        command_rx: mpsc::Receiver<HostCommand>,
    ) {
        set_string(&controller, &module_id, "status", "idle");
        set_string(&controller, &module_id, "last_error", "");

        let mut last_trigger = get_number(&controller, &module_id, "trigger_count") as u64;
        let mut last_reset = get_number(&controller, &module_id, "reset_count") as u64;
        let mut last_request_at: Option<Instant> = None;
        let mut history: Vec<Value> = Vec::new();

        loop {
            match command_rx.recv_timeout(Duration::from_millis(25)) {
                Ok(HostCommand::Stop) => {
                    set_string(&controller, &module_id, "status", "stopped");
                    return;
                }
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {}
            }

            let reset_count = get_number(&controller, &module_id, "reset_count") as u64;
            if reset_count != last_reset {
                last_reset = reset_count;
                history.clear();
                set_string(&controller, &module_id, "history_json", "[]");
                set_string(&controller, &module_id, "last_error", "");
                set_string(&controller, &module_id, "last_apply_error", "");
                set_string(&controller, &module_id, "status", "idle");
            }

            if !get_bool(&controller, &module_id, "enabled") {
                continue;
            }

            let trigger_count = get_number(&controller, &module_id, "trigger_count") as u64;
            if trigger_count == last_trigger {
                continue;
            }
            last_trigger = trigger_count;

            let cooldown_ms = get_number(&controller, &module_id, "cooldown_ms").max(0.0);
            if let Some(last) = last_request_at {
                if last.elapsed() < Duration::from_millis(cooldown_ms as u64) {
                    continue;
                }
            }
            last_request_at = Some(Instant::now());

            if let Err(error) = service_request(&module_id, &config, &controller, &mut history) {
                set_string(&controller, &module_id, "status", "error");
                set_string(&controller, &module_id, "last_error", &error);
            }
        }
    }

    /// Services one queued trigger for an agent module.
    ///
    /// The request lifecycle is: build context, call backend, parse and
    /// validate response, apply configured writes, then append bounded history.
    /// Any failure is surfaced via the module's status/error controls rather
    /// than panicking the runtime.
    fn service_request(
        module_id: &str,
        config: &Value,
        controller: &RuntimeController,
        history: &mut Vec<Value>,
    ) -> Result<(), String> {
        set_string(controller, module_id, "status", "building_context");
        set_string(controller, module_id, "last_error", "");
        set_string(controller, module_id, "last_apply_error", "");

        let limits = history_limits(config);
        let packet = build_request_packet(module_id, config, controller, history, &limits)?;

        set_string(controller, module_id, "status", "requesting");
        let backend = get_string(controller, module_id, "backend")
            .or_else(|| string_config(config, "backend"))
            .unwrap_or_else(|| "local:auto".to_string());
        let result = call_backend(&backend, config, &packet)?;

        set_string(controller, module_id, "status", "parsing");
        set_string(controller, module_id, "last_response", &result.text);

        let response_config = config.get("response").unwrap_or(&Value::Null);
        let format = response_config
            .get("format")
            .and_then(Value::as_str)
            .unwrap_or("text");
        let parsed = if format == "text" {
            None
        } else {
            Some(parse_json_response(&result.text)?)
        };

        if let Some(parsed) = &parsed {
            validate_response(response_config, parsed)?;
            set_string(
                controller,
                module_id,
                "last_response_json",
                &parsed.to_string(),
            );
            apply_response(module_id, config, controller, parsed)?;
        } else {
            set_string(controller, module_id, "last_response_json", "");
        }

        let entry = json!({
            "timestamp_ms": now_ms(),
            "request": packet,
            "response": {
                "text": result.text,
                "json": parsed,
                "backend": result.backend,
                "model": result.model
            }
        });
        history.push(entry);
        trim_history(history, &limits);
        set_string(
            controller,
            module_id,
            "history_json",
            &Value::Array(history.clone()).to_string(),
        );

        let count = get_number(controller, module_id, "request_count") as u64 + 1;
        let _ = controller.snapshot.set_control(
            module_id,
            "request_count",
            ControlValue::Number(count as f32),
        );
        set_string(controller, module_id, "status", "idle");
        Ok(())
    }

    /// Builds the structured packet sent to local/API backends.
    ///
    /// The packet separates task text, explicit context bindings, optional
    /// graph summary, bounded history, and response instructions so adapters do
    /// not need to scrape prose to recover structured data.
    fn build_request_packet(
        module_id: &str,
        config: &Value,
        controller: &RuntimeController,
        history: &[Value],
        limits: &HistoryLimits,
    ) -> Result<Value, String> {
        let prompt = get_string(controller, module_id, "prompt")
            .or_else(|| string_config(config, "prompt"))
            .unwrap_or_default();
        let system = get_string(controller, module_id, "system")
            .or_else(|| string_config(config, "system"))
            .unwrap_or_default();

        let mut packet = json!({
            "module_id": module_id,
            "system": system,
            "task": prompt,
            "context": build_context(config, controller)?,
            "history": history_for_prompt(history, limits),
            "response": config.get("response").cloned().unwrap_or(Value::Null)
        });

        if config
            .get("include_graph_summary")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            packet["graph"] = graph_summary(controller);
        }

        Ok(packet)
    }

    /// Resolves explicit context bindings against the runtime.
    ///
    /// Supported sources are original module config, a single control, or a set
    /// of controls matched by exact key or simple suffix wildcard such as
    /// `degree.*`.
    fn build_context(config: &Value, controller: &RuntimeController) -> Result<Value, String> {
        let mut map = serde_json::Map::new();
        let Some(bindings) = config.get("context").and_then(Value::as_array) else {
            return Ok(Value::Object(map));
        };

        let modules = controller.snapshot.list_modules();
        for binding in bindings {
            let name = binding
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "agent context binding missing name".to_string())?;
            let module_id = binding
                .get("from")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("agent context binding '{}' missing from", name))?;
            let source = binding
                .get("source")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("agent context binding '{}' missing source", name))?;

            let value = match source {
                "config" => {
                    let module = modules
                        .iter()
                        .find(|module| module.id == module_id)
                        .ok_or_else(|| format!("unknown context module '{}'", module_id))?;
                    if let Some(path) = binding.get("path").and_then(Value::as_str) {
                        get_path(&module.config, path).cloned().ok_or_else(|| {
                            format!("config path '{}' not found on '{}'", path, module_id)
                        })?
                    } else {
                        module.config.clone()
                    }
                }
                "control" => {
                    let key = binding
                        .get("key")
                        .and_then(Value::as_str)
                        .ok_or_else(|| format!("control binding '{}' missing key", name))?;
                    control_value_to_json(
                        controller
                            .snapshot
                            .get_control(module_id, key)
                            .map_err(|e| e.to_string())?,
                    )
                }
                "controls" => {
                    let keys = binding
                        .get("keys")
                        .and_then(Value::as_array)
                        .ok_or_else(|| format!("controls binding '{}' missing keys", name))?;
                    let controls = controller
                        .snapshot
                        .list_controls(Some(module_id))
                        .map_err(|e| e.to_string())?;
                    let available = controls
                        .first()
                        .map(|(_, controls)| controls.clone())
                        .unwrap_or_default();
                    let mut controls_map = serde_json::Map::new();
                    for wanted in keys.iter().filter_map(Value::as_str) {
                        for control in &available {
                            if key_matches(wanted, &control.key) {
                                let value = controller
                                    .snapshot
                                    .get_control(module_id, &control.key)
                                    .map_err(|e| e.to_string())?;
                                controls_map
                                    .insert(control.key.clone(), control_value_to_json(value));
                            }
                        }
                    }
                    Value::Object(controls_map)
                }
                other => return Err(format!("unknown agent context source '{}'", other)),
            };
            map.insert(name.to_string(), value);
        }

        Ok(Value::Object(map))
    }

    fn graph_summary(controller: &RuntimeController) -> Value {
        json!({
            "status": controller.snapshot.status(),
            "modules": controller.snapshot.list_modules().into_iter().map(|module| {
                json!({ "id": module.id, "type": module.module_type })
            }).collect::<Vec<_>>(),
            "connections": controller.snapshot.list_connections().into_iter().map(|conn| {
                json!({
                    "from": format!("{}:{}", conn.from, conn.from_port),
                    "to": format!("{}:{}", conn.to, conn.to_port)
                })
            }).collect::<Vec<_>>()
        })
    }

    /// Dispatches a request packet to the configured backend.
    ///
    /// `test:*` backends are deterministic in-process fakes for tests,
    /// `local_command` reads config-defined commands over stdin/stdout,
    /// `local:*` uses named local harness presets such as `local:claude` or
    /// `local:codex`, and provider backends use API keys from the environment.
    fn call_backend(
        backend: &str,
        config: &Value,
        packet: &Value,
    ) -> Result<BackendResult, String> {
        if backend.starts_with("test:") {
            let text = match config.get("test_response") {
                Some(Value::String(text)) => text.clone(),
                Some(value) => value.to_string(),
                None => packet.to_string(),
            };
            return Ok(BackendResult {
                text,
                backend: backend.to_string(),
                model: None,
            });
        }

        match backend {
            "local_command" => call_local_command(config, packet),
            backend if backend.starts_with("local:") => call_local_harness_backend(backend, packet),
            "openai" | "anthropic" | "provider_api" => call_provider_api(backend, config, packet),
            other => Err(format!("unknown agent backend '{}'", other)),
        }
    }

    fn call_local_command(config: &Value, packet: &Value) -> Result<BackendResult, String> {
        let command = config
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| "local_command backend requires config.command".to_string())?;
        let args: Vec<String> = config
            .get("args")
            .and_then(Value::as_array)
            .map(|args| {
                args.iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();
        run_command(command, &args, Some(&packet.to_string()), "local_command")
    }

    fn call_local_harness_backend(backend: &str, packet: &Value) -> Result<BackendResult, String> {
        let harness_name = backend.trim_start_matches("local:");
        if harness_name == "auto" {
            for harness in local_harnesses().iter().copied() {
                if command_available(harness.command) {
                    return call_local_harness(harness, packet);
                }
            }
            let tried = local_harnesses()
                .iter()
                .map(|harness| harness.name)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "no supported local agent command found (tried {})",
                tried
            ));
        }

        let harness = local_harnesses()
            .iter()
            .copied()
            .find(|harness| harness.name == harness_name)
            .ok_or_else(|| format!("unknown local agent harness '{}'", harness_name))?;
        if !command_available(harness.command) {
            return Err(format!(
                "local agent harness '{}' is not available (missing command '{}')",
                harness.name, harness.command
            ));
        }
        call_local_harness(harness, packet)
    }

    fn call_local_harness(harness: LocalHarness, packet: &Value) -> Result<BackendResult, String> {
        let args = (harness.build_args)(packet);
        run_command(harness.command, &args, None, harness.name)
    }

    fn local_harnesses() -> &'static [LocalHarness] {
        &[
            LocalHarness {
                name: "claude",
                command: "claude",
                build_args: claude_args,
            },
            LocalHarness {
                name: "codex",
                command: "codex",
                build_args: codex_args,
            },
        ]
    }

    fn claude_args(packet: &Value) -> Vec<String> {
        vec![
            "-p".to_string(),
            packet.to_string(),
            "--output-format".to_string(),
            "text".to_string(),
        ]
    }

    fn codex_args(packet: &Value) -> Vec<String> {
        vec![
            "exec".to_string(),
            "--skip-git-repo-check".to_string(),
            packet.to_string(),
        ]
    }

    fn call_provider_api(
        backend: &str,
        config: &Value,
        packet: &Value,
    ) -> Result<BackendResult, String> {
        let provider = if backend == "provider_api" {
            config
                .get("provider")
                .and_then(Value::as_str)
                .ok_or_else(|| "provider_api backend requires config.provider".to_string())?
        } else {
            backend
        };
        match provider {
            "openai" => call_openai(config, packet),
            "anthropic" => call_anthropic(config, packet),
            other => Err(format!("unknown provider_api provider '{}'", other)),
        }
    }

    fn call_openai(config: &Value, packet: &Value) -> Result<BackendResult, String> {
        let key =
            std::env::var("OPENAI_API_KEY").map_err(|_| "OPENAI_API_KEY is not set".to_string())?;
        let model = config
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("gpt-5.4-mini");
        let input = format_prompt_packet(packet);
        let body = json!({
            "model": model,
            "input": input,
        });
        let response: Value = ureq::post("https://api.openai.com/v1/responses")
            .set("authorization", &format!("Bearer {}", key))
            .set("content-type", "application/json")
            .send_string(&body.to_string())
            .map_err(|err| format!("OpenAI request failed: {}", err))?
            .into_string()
            .map_err(|err| format!("OpenAI response was not text: {}", err))
            .and_then(|body| {
                serde_json::from_str(&body)
                    .map_err(|err| format!("OpenAI response was not JSON: {}", err))
            })?;
        let text = response
            .get("output_text")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| extract_openai_output_text(&response))
            .ok_or_else(|| "OpenAI response did not include output text".to_string())?;
        Ok(BackendResult {
            text,
            backend: "openai".to_string(),
            model: Some(model.to_string()),
        })
    }

    fn call_anthropic(config: &Value, packet: &Value) -> Result<BackendResult, String> {
        let key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| "ANTHROPIC_API_KEY is not set".to_string())?;
        let model = config
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("claude-sonnet-4-5");
        let max_tokens = config
            .get("max_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(1024);
        let body = json!({
            "model": model,
            "max_tokens": max_tokens,
            "system": packet.get("system").and_then(Value::as_str).unwrap_or(""),
            "messages": [
                {
                    "role": "user",
                    "content": format_prompt_packet(packet)
                }
            ]
        });
        let response: Value = ureq::post("https://api.anthropic.com/v1/messages")
            .set("x-api-key", &key)
            .set("anthropic-version", "2023-06-01")
            .set("content-type", "application/json")
            .send_string(&body.to_string())
            .map_err(|err| format!("Anthropic request failed: {}", err))?
            .into_string()
            .map_err(|err| format!("Anthropic response was not text: {}", err))
            .and_then(|body| {
                serde_json::from_str(&body)
                    .map_err(|err| format!("Anthropic response was not JSON: {}", err))
            })?;
        let text = response
            .get("content")
            .and_then(Value::as_array)
            .and_then(|content| content.iter().find_map(|item| item.get("text")?.as_str()))
            .ok_or_else(|| "Anthropic response did not include text content".to_string())?
            .to_string();
        Ok(BackendResult {
            text,
            backend: "anthropic".to_string(),
            model: Some(model.to_string()),
        })
    }

    fn format_prompt_packet(packet: &Value) -> String {
        let mut prompt = String::new();
        if let Some(task) = packet.get("task").and_then(Value::as_str) {
            prompt.push_str(task);
            prompt.push_str("\n\n");
        }
        prompt.push_str("Context packet:\n");
        prompt.push_str(&packet.to_string());
        prompt
    }

    fn extract_openai_output_text(response: &Value) -> Option<String> {
        let mut parts = Vec::new();
        for output in response.get("output")?.as_array()? {
            for content in output.get("content")?.as_array()? {
                if let Some(text) = content.get("text").and_then(Value::as_str) {
                    parts.push(text.to_string());
                }
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(""))
        }
    }

    fn run_command(
        command: &str,
        args: &[String],
        stdin_payload: Option<&str>,
        backend: &str,
    ) -> Result<BackendResult, String> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(if stdin_payload.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("failed to spawn '{}': {}", command, err))?;

        if let Some(payload) = stdin_payload {
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(payload.as_bytes())
                    .map_err(|err| format!("failed to write to '{}': {}", command, err))?;
            }
        }

        let output = child
            .wait_with_output()
            .map_err(|err| format!("failed to wait for '{}': {}", command, err))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("{} failed: {}", command, stderr.trim()));
        }
        Ok(BackendResult {
            text: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            backend: backend.to_string(),
            model: None,
        })
    }

    fn command_available(command: &str) -> bool {
        Command::new(command)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn parse_json_response(text: &str) -> Result<Value, String> {
        serde_json::from_str(text).or_else(|_| {
            let start = text
                .find('{')
                .ok_or_else(|| "response is not JSON".to_string())?;
            let end = text
                .rfind('}')
                .ok_or_else(|| "response is not JSON".to_string())?;
            serde_json::from_str(&text[start..=end]).map_err(|err| err.to_string())
        })
    }

    /// Validates the parsed response before any graph writes occur.
    ///
    /// For `json` responses, Fugue expects the standard envelope
    /// `{ kind, summary, payload, confidence, warnings }`. Full JSON Schema
    /// validation is intentionally not implemented yet; v1 validates the
    /// built-in `fugue.step_pattern.v1` preset and preserves user schemas in the
    /// request packet for backend-side structured output.
    fn validate_response(response_config: &Value, response: &Value) -> Result<(), String> {
        let format = response_config
            .get("format")
            .and_then(Value::as_str)
            .unwrap_or("text");
        if format == "raw_json" {
            return Ok(());
        }

        for key in ["kind", "summary", "payload", "confidence", "warnings"] {
            if response.get(key).is_none() {
                return Err(format!(
                    "agent JSON response missing envelope key '{}'",
                    key
                ));
            }
        }

        if response_config.get("schema_ref").and_then(Value::as_str)
            == Some("fugue.step_pattern.v1")
        {
            validate_step_pattern(
                response
                    .get("payload")
                    .and_then(|payload| payload.get("pattern"))
                    .ok_or_else(|| "step pattern response missing payload.pattern".to_string())?,
            )?;
        }
        Ok(())
    }

    fn validate_step_pattern(pattern: &Value) -> Result<(), String> {
        let steps = pattern
            .as_array()
            .ok_or_else(|| "payload.pattern must be an array".to_string())?;
        if steps.is_empty() || steps.len() > 64 {
            return Err("payload.pattern length must be 1..=64".to_string());
        }
        for step in steps {
            let Some(object) = step.as_object() else {
                return Err("each pattern step must be an object".to_string());
            };
            match object.get("note") {
                Some(Value::Null) => {}
                Some(Value::Number(number)) if number.as_i64().is_some() => {}
                _ => return Err("step.note must be an integer or null".to_string()),
            }
            if let Some(gate) = object.get("gate").and_then(Value::as_f64) {
                if !(0.0..=1.0).contains(&gate) {
                    return Err("step.gate must be between 0 and 1".to_string());
                }
            }
        }
        Ok(())
    }

    /// Applies validated JSON response fields to configured target controls.
    ///
    /// Writes are explicit and opt-in. The agent never infers graph mutations
    /// from model output; each write must name a source JSON path, destination
    /// module, control key, and value type.
    fn apply_response(
        module_id: &str,
        config: &Value,
        controller: &RuntimeController,
        response: &Value,
    ) -> Result<(), String> {
        let Some(mappings) = config.get("apply").and_then(Value::as_array) else {
            return Ok(());
        };
        for mapping in mappings {
            let path = mapping
                .get("from")
                .and_then(Value::as_str)
                .ok_or_else(|| "apply mapping missing from".to_string())?;
            let target = mapping
                .get("to")
                .and_then(Value::as_str)
                .ok_or_else(|| "apply mapping missing to".to_string())?;
            let control = mapping
                .get("control")
                .and_then(Value::as_str)
                .ok_or_else(|| "apply mapping missing control".to_string())?;
            let write_type = mapping
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("string");
            let value = get_json_path(response, path)
                .ok_or_else(|| format!("apply path '{}' not found", path))?;
            let control_value = match write_type {
                "number" => {
                    let mut number = value
                        .as_f64()
                        .ok_or_else(|| format!("apply path '{}' is not a number", path))?
                        as f32;
                    if let Some(min) = mapping.get("min").and_then(Value::as_f64) {
                        number = number.max(min as f32);
                    }
                    if let Some(max) = mapping.get("max").and_then(Value::as_f64) {
                        number = number.min(max as f32);
                    }
                    ControlValue::Number(number)
                }
                "bool" => ControlValue::Bool(
                    value
                        .as_bool()
                        .ok_or_else(|| format!("apply path '{}' is not a bool", path))?,
                ),
                "json_string" => ControlValue::String(value.to_string()),
                "string" => ControlValue::String(
                    value
                        .as_str()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| value.to_string()),
                ),
                other => return Err(format!("unknown apply type '{}'", other)),
            };
            controller
                .snapshot
                .set_control(target, control, control_value)
                .map_err(|err| {
                    let message = err.to_string();
                    set_string(controller, module_id, "last_apply_error", &message);
                    message
                })?;
        }
        Ok(())
    }

    fn get_number(controller: &RuntimeController, module_id: &str, key: &str) -> f32 {
        match controller.snapshot.get_control(module_id, key) {
            Ok(ControlValue::Number(value)) => value,
            _ => 0.0,
        }
    }

    fn get_bool(controller: &RuntimeController, module_id: &str, key: &str) -> bool {
        match controller.snapshot.get_control(module_id, key) {
            Ok(ControlValue::Bool(value)) => value,
            _ => false,
        }
    }

    fn get_string(controller: &RuntimeController, module_id: &str, key: &str) -> Option<String> {
        match controller.snapshot.get_control(module_id, key) {
            Ok(ControlValue::String(value)) => Some(value),
            _ => None,
        }
    }

    fn set_string(controller: &RuntimeController, module_id: &str, key: &str, value: &str) {
        let _ = controller.snapshot.set_control(
            module_id,
            key,
            ControlValue::String(value.to_string()),
        );
    }

    fn control_value_to_json(value: ControlValue) -> Value {
        match value {
            ControlValue::Number(value) => json!(value),
            ControlValue::Bool(value) => json!(value),
            ControlValue::String(value) => json!(value),
        }
    }

    fn string_config(config: &Value, key: &str) -> Option<String> {
        config
            .get(key)
            .and_then(Value::as_str)
            .map(ToString::to_string)
    }

    fn history_limits(config: &Value) -> HistoryLimits {
        let history = config.get("history").unwrap_or(&Value::Null);
        HistoryLimits {
            max_turns: history
                .get("max_turns")
                .and_then(Value::as_u64)
                .unwrap_or(6) as usize,
            max_chars: history
                .get("max_chars")
                .and_then(Value::as_u64)
                .unwrap_or(12_000) as usize,
        }
    }

    fn history_for_prompt(history: &[Value], limits: &HistoryLimits) -> Value {
        let mut items: Vec<Value> = history
            .iter()
            .rev()
            .take(limits.max_turns)
            .cloned()
            .collect();
        items.reverse();
        Value::Array(items)
    }

    fn trim_history(history: &mut Vec<Value>, limits: &HistoryLimits) {
        while history.len() > limits.max_turns {
            history.remove(0);
        }
        while Value::Array(history.clone()).to_string().len() > limits.max_chars
            && !history.is_empty()
        {
            history.remove(0);
        }
    }

    fn get_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
        let mut current = value;
        for part in path.trim_start_matches("$.").split('.') {
            if part.is_empty() {
                continue;
            }
            current = current.get(part)?;
        }
        Some(current)
    }

    fn get_json_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
        get_path(value, path)
    }

    fn key_matches(pattern: &str, key: &str) -> bool {
        if let Some(prefix) = pattern.strip_suffix('*') {
            key.starts_with(prefix)
        } else {
            key == pattern
        }
    }

    fn now_ms() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0)
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use crate::invention::{RuntimeController, RuntimeModuleInfo};

    #[derive(Default)]
    pub struct AgentManager;

    impl AgentManager {
        pub fn start_all(&self, _controller: RuntimeController) {}
        pub fn start_module(&self, _controller: RuntimeController, _module: RuntimeModuleInfo) {}
        pub fn stop_module(&self, _module_id: &str) {}
        pub fn stop_all(&self) {}
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::AgentManager;
#[cfg(target_arch = "wasm32")]
pub use wasm::AgentManager;
