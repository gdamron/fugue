use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsResult, JsString, JsValue, NativeFunction, Source};
use serde_json::json;

use crate::invention::{RuntimeController, RuntimeModuleInfo};
use crate::ControlValue;

enum HostCommand {
    Stop,
    Reset,
}

struct NativeScriptHost {
    command_tx: Sender<HostCommand>,
    join: JoinHandle<()>,
}

#[derive(Default)]
pub struct ScriptManager {
    hosts: Mutex<HashMap<String, NativeScriptHost>>,
}

impl ScriptManager {
    pub fn start_all(&self, controller: RuntimeController) {
        for module in controller.snapshot.list_modules() {
            if module.module_type == "code" {
                self.start_module(controller.clone(), module);
            }
        }
    }

    pub fn start_module(&self, controller: RuntimeController, module: RuntimeModuleInfo) {
        if module.module_type != "code" {
            return;
        }

        self.stop_module(&module.id);

        let module_id = module.id.clone();
        let (command_tx, command_rx) = mpsc::channel();
        let join = thread::spawn(move || {
            run_host(module_id, module.config, controller, command_rx);
        });

        self.hosts
            .lock()
            .unwrap()
            .insert(module.id, NativeScriptHost { command_tx, join });
    }

    pub fn reset_module(&self, module_id: &str) {
        if let Some(host) = self.hosts.lock().unwrap().get(module_id) {
            let _ = host.command_tx.send(HostCommand::Reset);
        }
    }

    pub fn stop_module(&self, module_id: &str) {
        if let Some(host) = self.hosts.lock().unwrap().remove(module_id) {
            let _ = host.command_tx.send(HostCommand::Stop);
            let _ = host.join.join();
        }
    }

    pub fn stop_all(&self) {
        let module_ids: Vec<String> = self.hosts.lock().unwrap().keys().cloned().collect();
        for module_id in module_ids {
            self.stop_module(&module_id);
        }
    }
}

fn run_host(
    module_id: String,
    config: serde_json::Value,
    controller: RuntimeController,
    command_rx: mpsc::Receiver<HostCommand>,
) {
    set_status(&controller, &module_id, "starting");
    clear_error(&controller, &module_id);

    let mut context = Context::default();
    if let Err(err) = install_graph_api(&module_id, &controller, &mut context) {
        set_error(&controller, &module_id, &err);
        return;
    }

    let script = config
        .get("script")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let entrypoint = config
        .get("entrypoint")
        .and_then(|value| value.as_str())
        .unwrap_or("init")
        .to_string();

    if let Err(err) = context.eval(Source::from_bytes(script.as_bytes())) {
        set_error(&controller, &module_id, &err.to_string());
        return;
    }

    if let Err(err) = call_hook(&entrypoint, &mut context) {
        set_error(&controller, &module_id, &err.to_string());
        return;
    }
    set_status(&controller, &module_id, "running");

    loop {
        let enabled = matches!(
            controller.snapshot.get_control(&module_id, "enabled"),
            Ok(ControlValue::Bool(true))
        );
        let tick_hz = match controller.snapshot.get_control(&module_id, "tick_hz") {
            Ok(ControlValue::Number(value)) => value.max(0.0),
            _ => 0.0,
        };
        let timeout = if enabled && tick_hz > 0.0 {
            Duration::from_secs_f32(1.0 / tick_hz)
        } else {
            Duration::from_millis(250)
        };

        match command_rx.recv_timeout(timeout) {
            Ok(HostCommand::Stop) => {
                set_status(&controller, &module_id, "stopped");
                return;
            }
            Ok(HostCommand::Reset) => {
                clear_error(&controller, &module_id);
                if let Err(err) = call_hook("reset", &mut context) {
                    set_error(&controller, &module_id, &err.to_string());
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if enabled && tick_hz > 0.0 {
                    if let Err(err) = call_hook("tick", &mut context) {
                        set_error(&controller, &module_id, &err.to_string());
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn install_graph_api(
    module_id: &str,
    controller: &RuntimeController,
    context: &mut Context,
) -> Result<(), String> {
    let status_controller = controller.clone();
    let status_fn = unsafe {
        NativeFunction::from_closure(move |_this, _args, context| {
            json_to_js(&json!(status_controller.snapshot.status()), context)
        })
    };

    let list_modules_controller = controller.clone();
    let list_modules_fn = unsafe {
        NativeFunction::from_closure(move |_this, _args, context| {
            json_to_js(
                &json!(list_modules_controller.snapshot.list_modules()),
                context,
            )
        })
    };

    let list_connections_controller = controller.clone();
    let list_connections_fn = unsafe {
        NativeFunction::from_closure(move |_this, _args, context| {
            json_to_js(
                &json!(list_connections_controller.snapshot.list_connections()),
                context,
            )
        })
    };

    let list_controls_controller = controller.clone();
    let list_controls_fn =
        unsafe {
            NativeFunction::from_closure(move |_this, args, context| {
                let module_id = optional_string_arg(args.first(), context)?;
                let value = list_controls_controller
                    .snapshot
                    .list_controls(module_id.as_deref())
                    .map_err(|err| js_err(err.to_string()))?;
                let payload: Vec<_> = value
            .into_iter()
            .map(|(module_id, controls)| json!({ "module_id": module_id, "controls": controls }))
            .collect();
                json_to_js(&serde_json::Value::Array(payload), context)
            })
        };

    let get_control_controller = controller.clone();
    let get_control_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, context| {
            let module_id = string_arg(args.first(), context, "module_id")?;
            let key = string_arg(args.get(1), context, "key")?;
            let value = get_control_controller
                .snapshot
                .get_control(&module_id, &key)
                .map_err(|err| js_err(err.to_string()))?;
            control_to_js(value, context)
        })
    };

    let set_control_controller = controller.clone();
    let set_control_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, context| {
            let module_id = string_arg(args.first(), context, "module_id")?;
            let key = string_arg(args.get(1), context, "key")?;
            let value = control_from_js(args.get(2), context)?;
            set_control_controller
                .snapshot
                .set_control(&module_id, &key, value)
                .map_err(|err| js_err(err.to_string()))?;
            Ok(JsValue::undefined())
        })
    };

    let add_module_controller = controller.clone();
    let add_module_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, context| {
            let module_id = string_arg(args.first(), context, "module_id")?;
            let module_type = string_arg(args.get(1), context, "module_type")?;
            let config = json_arg(args.get(2), context)?;
            add_module_controller
                .add_module(&module_id, &module_type, &config)
                .map_err(|err| js_err(err.to_string()))?;
            Ok(JsValue::undefined())
        })
    };

    let remove_module_controller = controller.clone();
    let remove_module_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, context| {
            let module_id = string_arg(args.first(), context, "module_id")?;
            remove_module_controller
                .remove_module(&module_id)
                .map_err(|err| js_err(err.to_string()))?;
            Ok(JsValue::undefined())
        })
    };

    let connect_controller = controller.clone();
    let connect_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, context| {
            let from = string_arg(args.first(), context, "from")?;
            let from_port = string_arg(args.get(1), context, "from_port")?;
            let to = string_arg(args.get(2), context, "to")?;
            let to_port = string_arg(args.get(3), context, "to_port")?;
            connect_controller
                .connect(&from, &from_port, &to, &to_port)
                .map_err(|err| js_err(err.to_string()))?;
            Ok(JsValue::undefined())
        })
    };

    let disconnect_controller = controller.clone();
    let disconnect_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, context| {
            let from = string_arg(args.first(), context, "from")?;
            let from_port = string_arg(args.get(1), context, "from_port")?;
            let to = string_arg(args.get(2), context, "to")?;
            let to_port = string_arg(args.get(3), context, "to_port")?;
            disconnect_controller
                .disconnect(&from, &from_port, &to, &to_port)
                .map_err(|err| js_err(err.to_string()))?;
            Ok(JsValue::undefined())
        })
    };

    let fetch_fn = unsafe {
        NativeFunction::from_closure(move |_this, args, context| {
            let url = string_arg(args.first(), context, "url")?;
            let response = ureq::get(&url)
                .call()
                .map_err(|err| js_err(err.to_string()))?;
            let status = response.status();
            let text = response
                .into_string()
                .map_err(|err| js_err(err.to_string()))?;
            json_to_js(
                &json!({ "ok": status < 400, "status": status, "text": text }),
                context,
            )
        })
    };

    let graph = ObjectInitializer::new(context)
        .function(status_fn, js_string!("status"), 0)
        .function(list_modules_fn, js_string!("listModules"), 0)
        .function(list_connections_fn, js_string!("listConnections"), 0)
        .function(list_controls_fn, js_string!("listControls"), 1)
        .function(get_control_fn, js_string!("getControl"), 2)
        .function(set_control_fn, js_string!("setControl"), 3)
        .function(add_module_fn, js_string!("addModule"), 3)
        .function(remove_module_fn, js_string!("removeModule"), 1)
        .function(connect_fn, js_string!("connect"), 4)
        .function(disconnect_fn, js_string!("disconnect"), 4)
        .function(fetch_fn, js_string!("fetch"), 1)
        .property(
            js_string!("moduleId"),
            JsString::from(module_id),
            Attribute::all(),
        )
        .build();

    context
        .register_global_property(js_string!("graph"), graph, Attribute::all())
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn call_hook(name: &str, context: &mut Context) -> Result<(), String> {
    let escaped = name.replace('\\', "\\\\").replace('\'', "\\'");
    context
        .eval(Source::from_bytes(
            format!(
                "typeof globalThis['{escaped}'] === 'function' ? globalThis['{escaped}']() : undefined"
            )
            .as_bytes(),
        ))
        .map(|_| ())
        .map_err(|err| err.to_string())
}

fn string_arg(value: Option<&JsValue>, context: &mut Context, name: &str) -> JsResult<String> {
    let value = value.ok_or_else(|| js_err(format!("missing {}", name)))?;
    value
        .to_string(context)
        .map(|value| value.to_std_string_escaped())
}

fn optional_string_arg(value: Option<&JsValue>, context: &mut Context) -> JsResult<Option<String>> {
    match value {
        None => Ok(None),
        Some(value) if value.is_undefined() || value.is_null() => Ok(None),
        Some(value) => value
            .to_string(context)
            .map(|value| Some(value.to_std_string_escaped())),
    }
}

fn json_arg(value: Option<&JsValue>, context: &mut Context) -> JsResult<serde_json::Value> {
    match value {
        None => Ok(serde_json::Value::Null),
        Some(value) => Ok(value.to_json(context)?.unwrap_or(serde_json::Value::Null)),
    }
}

fn control_from_js(value: Option<&JsValue>, context: &mut Context) -> JsResult<ControlValue> {
    let value = value.ok_or_else(|| js_err("missing control value"))?;
    match value.to_json(context)?.unwrap_or(serde_json::Value::Null) {
        serde_json::Value::Bool(value) => Ok(ControlValue::Bool(value)),
        serde_json::Value::Number(value) => value
            .as_f64()
            .map(|value| ControlValue::Number(value as f32))
            .ok_or_else(|| js_err("control number must fit in f32")),
        serde_json::Value::String(value) => Ok(ControlValue::String(value)),
        _ => Err(js_err("control value must be boolean, number, or string")),
    }
}

fn control_to_js(value: ControlValue, _context: &mut Context) -> JsResult<JsValue> {
    match value {
        ControlValue::Bool(value) => Ok(JsValue::from(value)),
        ControlValue::Number(value) => Ok(JsValue::from(value)),
        ControlValue::String(value) => Ok(JsValue::from(JsString::from(value))),
    }
}

fn json_to_js(value: &serde_json::Value, context: &mut Context) -> JsResult<JsValue> {
    JsValue::from_json(value, context)
}

fn js_err(message: impl Into<String>) -> boa_engine::JsError {
    boa_engine::JsError::from_opaque(JsValue::from(JsString::from(message.into())))
}

fn set_status(controller: &RuntimeController, module_id: &str, status: &str) {
    let _ = controller.snapshot.set_control(
        module_id,
        "status",
        ControlValue::String(status.to_string()),
    );
}

fn clear_error(controller: &RuntimeController, module_id: &str) {
    let _ = controller.snapshot.set_control(
        module_id,
        "last_error",
        ControlValue::String(String::new()),
    );
}

fn set_error(controller: &RuntimeController, module_id: &str, error: &str) {
    set_status(controller, module_id, "error");
    let _ = controller.snapshot.set_control(
        module_id,
        "last_error",
        ControlValue::String(error.to_string()),
    );
}
