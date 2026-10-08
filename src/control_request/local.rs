//! A declared module's own `f32` control API (`Module::controls`,
//! `get_control`, `set_control`), for using a module directly, outside any
//! graph: tests, and code embedding one module. A choice is its position,
//! a boolean is 0 or 1.

use super::cells::apply_declared;
use super::declare::DeclKind;
use super::request::RtValue;
use crate::{ControlMeta, Module};

/// The module's declared controls, each with what it holds now.
pub(crate) fn local_controls<M: Module + ?Sized>(module: &M) -> Vec<ControlMeta> {
    let Some((table, cells)) = module.declared() else {
        return Vec::new();
    };
    table.metas(|index| table.value(index, cells.load(index)?))
}

/// What `key` holds, as a number.
pub(crate) fn local_get<M: Module + ?Sized>(module: &M, key: &str) -> Result<f32, String> {
    let unknown = || format!("Unknown control: {key}");
    let (table, cells) = module.declared().ok_or_else(unknown)?;
    let index = table.resolve(key).ok_or_else(unknown)?;
    Ok(match cells.load(index).ok_or_else(unknown)? {
        RtValue::F32(value) => value,
        RtValue::I32(value) => value as f32,
        RtValue::U32(value) => value as f32,
        RtValue::Bool(value) => f32::from(u8::from(value)),
    })
}

/// Sets `key` from a number, applied at once.
pub(crate) fn local_set<M: Module + ?Sized>(
    module: &mut M,
    key: &str,
    value: f32,
) -> Result<(), String> {
    let unknown = || format!("Unknown control: {key}");
    let (table, _) = module.declared().ok_or_else(unknown)?;
    let index = table.resolve(key).ok_or_else(unknown)?;
    let (decl, _) = table.decl(index).ok_or_else(unknown)?;
    let whole = value.fract() == 0.0 && value.is_finite();
    let value = match decl.kind {
        DeclKind::Number { .. } if value.is_finite() => RtValue::F32(value),
        DeclKind::Integer { .. } if whole => RtValue::I32(value as i32),
        DeclKind::Choice(_) if whole && value >= 0.0 => RtValue::U32(value as u32),
        DeclKind::Bool => RtValue::Bool(value != 0.0),
        _ => return Err(format!("Control '{key}' cannot hold {value}")),
    };
    apply_declared(module, index, value).map_err(|refusal| format!("Control '{key}': {refusal:?}"))
}
