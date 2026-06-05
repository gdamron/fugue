use super::*;

#[derive(Clone, Copy)]
pub(super) enum ConsoleSink {
    Stdout,
    Stderr,
}

pub(super) fn make_console_writer(module_id: &str, sink: ConsoleSink, level: &'static str) -> NativeFunction {
    let module_id = module_id.to_string();
    unsafe {
        NativeFunction::from_closure(move |_this, args, context| {
            let mut buf = String::new();
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    buf.push(' ');
                }
                let rendered = arg
                    .to_string(context)?
                    .to_std_string_escaped();
                buf.push_str(&rendered);
            }
            let line = format!("[{} {}] {}", module_id, level, buf);
            match sink {
                ConsoleSink::Stdout => println!("{}", line),
                ConsoleSink::Stderr => eprintln!("{}", line),
            }
            Ok(JsValue::undefined())
        })
    }
}

pub(super) fn call_hook(name: &str, context: &mut Context) -> Result<bool, String> {
    let escaped = name.replace('\\', "\\\\").replace('\'', "\\'");
    context
        .eval(Source::from_bytes(
            format!(
                r#"(() => {{
                    let __fugue_hook;
                    if (
                        typeof __fugue_result === 'object' &&
                        __fugue_result !== null &&
                        typeof __fugue_result['{escaped}'] === 'function'
                    ) {{
                        __fugue_hook = __fugue_result['{escaped}'];
                    }} else if (typeof globalThis['{escaped}'] === 'function') {{
                        __fugue_hook = globalThis['{escaped}'];
                    }}

                    if (typeof __fugue_hook === 'function') {{
                        __fugue_hook();
                        return true;
                    }}

                    return false;
                }})()"#
            )
            .as_bytes(),
        ))
        .map_err(|err| err.to_string())
        .map(|value| value.to_boolean())
}

pub(super) fn string_arg(value: Option<&JsValue>, context: &mut Context, name: &str) -> JsResult<String> {
    let value = value.ok_or_else(|| js_err(format!("missing {}", name)))?;
    value
        .to_string(context)
        .map(|value| value.to_std_string_escaped())
}

pub(super) fn optional_string_arg(value: Option<&JsValue>, context: &mut Context) -> JsResult<Option<String>> {
    match value {
        None => Ok(None),
        Some(value) if value.is_undefined() || value.is_null() => Ok(None),
        Some(value) => value
            .to_string(context)
            .map(|value| Some(value.to_std_string_escaped())),
    }
}

pub(super) fn json_arg(value: Option<&JsValue>, context: &mut Context) -> JsResult<serde_json::Value> {
    match value {
        None => Ok(serde_json::Value::Null),
        Some(value) => Ok(value.to_json(context)?.unwrap_or(serde_json::Value::Null)),
    }
}

pub(super) fn control_from_js(value: Option<&JsValue>, context: &mut Context) -> JsResult<ControlValue> {
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

pub(super) fn control_to_js(value: ControlValue, _context: &mut Context) -> JsResult<JsValue> {
    match value {
        ControlValue::Bool(value) => Ok(JsValue::from(value)),
        ControlValue::Number(value) => Ok(JsValue::from(value)),
        ControlValue::String(value) => Ok(JsValue::from(JsString::from(value))),
    }
}

pub(super) fn json_to_js(value: &serde_json::Value, context: &mut Context) -> JsResult<JsValue> {
    JsValue::from_json(value, context)
}

pub(super) fn js_err(message: impl Into<String>) -> boa_engine::JsError {
    boa_engine::JsError::from_opaque(JsValue::from(JsString::from(message.into())))
}

pub(super) fn set_status(controller: &RuntimeController, module_id: &str, status: &str) {
    let _ = controller.snapshot.set_control(
        module_id,
        "status",
        ControlValue::String(status.to_string()),
    );
}

pub(super) fn clear_error(controller: &RuntimeController, module_id: &str) {
    let _ = controller.snapshot.set_control(
        module_id,
        "last_error",
        ControlValue::String(String::new()),
    );
}

pub(super) fn set_error(controller: &RuntimeController, module_id: &str, error: &str) {
    set_status(controller, module_id, "error");
    let _ = controller.snapshot.set_control(
        module_id,
        "last_error",
        ControlValue::String(error.to_string()),
    );
}
