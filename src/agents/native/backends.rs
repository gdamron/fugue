use super::*;

#[derive(Clone, Debug)]
pub(super) struct BackendResult {
    pub(super) text: String,
    pub(super) backend: String,
    pub(super) model: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct LocalHarness {
    name: &'static str,
    command: &'static str,
    build_args: fn(&Value, &Value) -> Vec<String>,
}


/// Dispatches a request packet to the configured backend.
///
/// `test:*` backends are deterministic in-process fakes for tests,
/// `local_command` reads config-defined commands over stdin/stdout,
/// `local:*` uses named local harness presets such as `local:claude` or
/// `local:codex`, and provider backends use API keys from the environment.
pub(super) fn call_backend(
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
        backend if backend.starts_with("local:") => {
            call_local_harness_backend(backend, config, packet)
        }
        "openai" | "anthropic" | "provider_api" => call_provider_api(backend, config, packet),
        other => Err(format!("unknown agent backend '{}'", other)),
    }
}

pub(super) fn call_local_command(config: &Value, packet: &Value) -> Result<BackendResult, String> {
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

pub(super) fn call_local_harness_backend(
    backend: &str,
    config: &Value,
    packet: &Value,
) -> Result<BackendResult, String> {
    let harness_name = backend.trim_start_matches("local:");
    if harness_name == "auto" {
        for harness in local_harnesses().iter().copied() {
            if command_available(harness.command) {
                eprintln!("[agent] auto-selected harness '{}'", harness.name);
                return call_local_harness(harness, config, packet);
            }
            eprintln!(
                "[agent] harness '{}' unavailable (command '{}' not found)",
                harness.name, harness.command
            );
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
    call_local_harness(harness, config, packet)
}

pub(super) fn call_local_harness(
    harness: LocalHarness,
    config: &Value,
    packet: &Value,
) -> Result<BackendResult, String> {
    let args = (harness.build_args)(packet, config);
    run_command(harness.command, &args, None, harness.name)
}

pub(super) fn local_harnesses() -> &'static [LocalHarness] {
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

pub(super) fn claude_args(packet: &Value, config: &Value) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        format_prompt_packet(packet),
        "--output-format".to_string(),
        "text".to_string(),
        "--no-session-persistence".to_string(),
        "--tools".to_string(),
        "".to_string(),
    ];
    if let Some(system) = packet.get("system").and_then(Value::as_str) {
        if !system.is_empty() {
            args.push("--system-prompt".to_string());
            args.push(system.to_string());
        }
    }
    if let Some(model) = config.get("model").and_then(Value::as_str) {
        args.push("--model".to_string());
        args.push(model.to_string());
    }
    args
}

pub(super) fn codex_args(packet: &Value, _config: &Value) -> Vec<String> {
    vec![
        "exec".to_string(),
        "--skip-git-repo-check".to_string(),
        packet.to_string(),
    ]
}

pub(super) fn call_provider_api(
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

pub(super) fn call_openai(config: &Value, packet: &Value) -> Result<BackendResult, String> {
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

pub(super) fn call_anthropic(config: &Value, packet: &Value) -> Result<BackendResult, String> {
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

pub(super) fn format_prompt_packet(packet: &Value) -> String {
    let mut prompt = String::new();
    if let Some(task) = packet.get("task").and_then(Value::as_str) {
        prompt.push_str(task);
        prompt.push_str("\n\n");
    }
    prompt.push_str("Context packet:\n");
    prompt.push_str(&packet.to_string());
    prompt
}

pub(super) fn extract_openai_output_text(response: &Value) -> Option<String> {
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

pub(super) fn run_command(
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

pub(super) fn command_available(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
