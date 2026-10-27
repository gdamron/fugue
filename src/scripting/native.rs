use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsResult, JsString, JsValue, NativeFunction, Source};
use serde_json::json;

use crate::control_request::Source as WriteSource;
use crate::invention::declared::WriteOptions;
use crate::invention::runtime::control_timing::timing_time;
use crate::invention::{RuntimeController, RuntimeModuleInfo};
use crate::{ControlValue, ControlWriteIntent};

mod js_interop;
use js_interop::*;

/// Fastest rate the host ticks a code module, matching the control's editor
/// range. A declared `tick_rate` above this is stored as written, but the host
/// ticks at this rate instead of busy-looping.
const MAX_TICK_RATE_HZ: f32 = 1000.0;

/// Longest the host waits between ticks. A positive rate too small to name a
/// representable period (such as `1e-38`) waits this long instead of panicking.
const MAX_TICK_PERIOD: Duration = Duration::from_secs(24 * 60 * 60);

/// Period between ticks for a positive `tick_rate`, bounded to
/// `1 / MAX_TICK_RATE_HZ` through `MAX_TICK_PERIOD`.
fn tick_period(tick_rate: f32) -> Duration {
    let rate = tick_rate.min(MAX_TICK_RATE_HZ);
    Duration::try_from_secs_f32(1.0 / rate)
        .unwrap_or(MAX_TICK_PERIOD)
        .min(MAX_TICK_PERIOD)
}

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

    match context.eval(Source::from_bytes(script.as_bytes())) {
        Ok(result) => {
            if let Err(err) = context.register_global_property(
                js_string!("__fugue_result"),
                result,
                Attribute::all(),
            ) {
                set_error(&controller, &module_id, &err.to_string());
                return;
            }
        }
        Err(err) => {
            set_error(&controller, &module_id, &err.to_string());
            return;
        }
    }

    match call_hook(&entrypoint, &mut context) {
        Ok(true) => {}
        Ok(false) if entrypoint != "init" => {
            if let Err(err) = call_hook("init", &mut context) {
                set_error(&controller, &module_id, &err);
                return;
            }
        }
        Ok(false) => {}
        Err(err) => {
            set_error(&controller, &module_id, &err);
            return;
        }
    }

    set_status(&controller, &module_id, "running");

    loop {
        let enabled = matches!(
            controller.snapshot.get_control(&module_id, "enabled"),
            Ok(ControlValue::Bool(true))
        );
        let tick_rate = match controller.snapshot.get_control(&module_id, "tick_rate") {
            Ok(ControlValue::Number(value)) => value.max(0.0),
            _ => 0.0,
        };
        let timeout = if enabled && tick_rate > 0.0 {
            tick_period(tick_rate)
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
                    set_error(&controller, &module_id, &err);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if enabled && tick_rate > 0.0 {
                    if let Err(err) = call_hook("tick", &mut context) {
                        set_error(&controller, &module_id, &err);
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
            let timing = timing_arg(args.get(3), context)?;
            let snapshot = &set_control_controller.snapshot;
            let time = timing_time(&timing).map_err(|err| js_err(err.to_string()))?;
            // A conducting script performs: its writes are live gestures, not
            // the invention's new starting state, so they stay out of the
            // retained document and out of the authoring revision. It ranks
            // as a script, below a person's or an agent's write.
            let options = WriteOptions {
                ttl: (timing.ttl_ms).map(|ms| {
                    ms.saturating_mul(u64::from(set_control_controller.sample_rate)) / 1000
                }),
                time,
                ..WriteOptions::performed(WriteSource::Script)
            };
            let perform = ControlWriteIntent::Perform;
            let ack = snapshot
                .submit_control(&module_id, &key, value, perform, options)
                .map_err(|err| js_err(err.to_string()))?;
            // Its request, for a later write to follow (`after_request`).
            Ok(ack.request.map_or(JsValue::undefined(), |id| {
                JsValue::from(JsString::from(id.0.to_string()))
            }))
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
            let mut response = crate::http::get(&url).map_err(js_err)?;
            let status = response.status().as_u16();
            let text = crate::http::read_text(response.body_mut()).map_err(js_err)?;
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
        .function(set_control_fn, js_string!("setControl"), 4)
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

    let event_sink = controller.snapshot.event_sink.clone();
    let log_fn = make_console_writer(module_id, ConsoleSink::Stdout, "log", event_sink.clone());
    let info_fn = make_console_writer(module_id, ConsoleSink::Stdout, "info", event_sink.clone());
    let warn_fn = make_console_writer(module_id, ConsoleSink::Stderr, "warn", event_sink.clone());
    let error_fn = make_console_writer(module_id, ConsoleSink::Stderr, "error", event_sink.clone());
    let debug_fn = make_console_writer(module_id, ConsoleSink::Stdout, "debug", event_sink);

    let console = ObjectInitializer::new(context)
        .function(log_fn, js_string!("log"), 0)
        .function(info_fn, js_string!("info"), 0)
        .function(warn_fn, js_string!("warn"), 0)
        .function(error_fn, js_string!("error"), 0)
        .function(debug_fn, js_string!("debug"), 0)
        .build();

    context
        .register_global_property(js_string!("console"), console, Attribute::all())
        .map_err(|err| err.to_string())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_period_follows_the_rate_within_bounds() {
        assert_eq!(tick_period(4.0), Duration::from_millis(250));
        assert_eq!(tick_period(MAX_TICK_RATE_HZ), Duration::from_millis(1));
    }

    #[test]
    fn tiny_tick_rate_waits_the_longest_period_instead_of_panicking() {
        assert_eq!(tick_period(1e-38), MAX_TICK_PERIOD);
        assert_eq!(tick_period(f32::MIN_POSITIVE), MAX_TICK_PERIOD);
        assert_eq!(tick_period(1e-9), MAX_TICK_PERIOD);
    }

    #[test]
    fn huge_tick_rate_ticks_no_faster_than_the_maximum_rate() {
        let fastest = Duration::from_secs_f32(1.0 / MAX_TICK_RATE_HZ);
        assert_eq!(tick_period(1e9), fastest);
        assert_eq!(tick_period(f32::MAX), fastest);
        assert_eq!(tick_period(f32::INFINITY), fastest);
    }
}
