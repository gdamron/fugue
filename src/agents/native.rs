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

mod backends;
mod context;
mod controls;
mod response;

use backends::*;
use context::*;
use controls::*;
use response::*;

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
            eprintln!("[agent:{}] error: {}", module_id, error);
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
    eprintln!("[agent:{}] requesting via backend '{}'", module_id, backend);
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
        Some(parse_json_response(&result.text).inspect_err(|_err| {
            let preview: String = result.text.chars().take(200).collect();
            eprintln!("[agent:{}] response preview: {}", module_id, preview);
        })?)
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
    eprintln!("[agent:{}] request #{} complete", module_id, count);
    set_string(controller, module_id, "status", "idle");
    Ok(())
}
