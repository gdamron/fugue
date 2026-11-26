//! The one rule a control write from a front door meets before it is made:
//! accepted as it is, set to the nearer limit of the range its control
//! holds, or refused with nothing written.
//!
//! A write is accepted in the loose forms a client may send (number text
//! for a number, any case for a choice). A number outside the range its
//! control holds is set to the nearer limit, and the write says it was
//! clamped. A write is refused when its module or control does not exist,
//! when its value is not of the control's kind or not a finite number, and
//! when the module's own setter rules refuse it (a read-only control, an
//! option the control does not offer). A batch is checked whole, before
//! any of it is written (see [`check_batch`]), so a refusal leaves nothing
//! written.
//!
//! Every check runs on the control thread, before a write is handed to the
//! audio thread.

use crate::invention::edits::refusals::{bounded, listing};
use crate::invention::orchestration::RuntimeSnapshot;
use crate::invention::runtime::control_timing::timing_time;
use crate::invention::runtime::GraphCommandError;
use crate::rpc::{
    non_finite_refusal, ControlWriteReason, ControlWriteRefusal, RpcError, MODULE_ERROR_BYTES,
};
use crate::traits::{exact_number, ControlSurfaceMap};
use crate::{ControlKind, ControlValue, ControlWrite};

/// The most of a refused value a refusal echoes, in bytes.
const ECHOED_VALUE_BYTES: usize = 64;

/// A write the rule accepts: the value to make, as coerced to its
/// control's kind and held within its control's range.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CheckedWrite {
    pub(crate) value: ControlValue,
    /// Whether the number written lay outside the range its control holds,
    /// so `value` is the nearer limit.
    pub(crate) clamped: bool,
}

/// Why the rule refuses a write.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WriteRefusal {
    pub(crate) reason: ControlWriteReason,
    pub(crate) message: String,
}

impl WriteRefusal {
    fn new(reason: ControlWriteReason, message: String) -> Self {
        Self { reason, message }
    }
}

impl RuntimeSnapshot {
    /// Checks every write of a batch as [`check_batch`] does, against the
    /// modules running now. Changes nothing.
    pub(crate) fn check_writes(
        &self,
        writes: &[ControlWrite],
    ) -> Result<Vec<CheckedWrite>, RpcError> {
        // Checked outside the directory's lock, as writes are made: a
        // scheduler's `schedule` resolves its targets against it.
        let directory: ControlSurfaceMap = self.control_surfaces.lock().unwrap().clone();
        let checked = check_batch(&directory, writes)?;
        // The timed among them, each as it would be taken now, together
        // filling the room timed writes wait in.
        let Some(live) = &self.live else {
            return Ok(checked);
        };
        let mut times = Vec::new();
        for (index, write) in writes.iter().enumerate() {
            if let Ok(Some(time)) = timing_time(&write.timing) {
                times.push((index, write.module_id.as_str(), time));
            }
        }
        if let Err((index, message)) = live.check_write_times(&times) {
            let write = &writes[index];
            return Err(RpcError::invalid_control_write(ControlWriteRefusal {
                index,
                module_id: write.module_id.clone(),
                key: write.key.clone(),
                reason: ControlWriteReason::InvalidTiming,
                message,
            }));
        }
        Ok(checked)
    }
}

/// Checks every write of a batch against `directory`, the modules' control
/// surfaces, in order, and its timing; each write as the rule takes it, or
/// the refusal of the first it refuses, naming that write. Changes nothing.
pub(crate) fn check_batch(
    directory: &ControlSurfaceMap,
    writes: &[ControlWrite],
) -> Result<Vec<CheckedWrite>, RpcError> {
    let refused = |index: usize, write: &ControlWrite, refusal: WriteRefusal| {
        RpcError::invalid_control_write(ControlWriteRefusal {
            index,
            module_id: write.module_id.clone(),
            key: write.key.clone(),
            reason: refusal.reason,
            message: refusal.message,
        })
    };
    let mut checked = Vec::with_capacity(writes.len());
    for (index, write) in writes.iter().enumerate() {
        let timed = match timing_time(&write.timing) {
            Ok(time) => time.is_some(),
            Err(error) => {
                let message = match error {
                    GraphCommandError::ControlError(message) => message,
                    other => other.to_string(),
                };
                let refusal = WriteRefusal::new(ControlWriteReason::InvalidTiming, message);
                return Err(refused(index, write, refusal));
            }
        };
        let (module_id, key) = (&write.module_id, &write.key);
        match check_write(directory, module_id, key, &write.value, timed) {
            Ok(write) => checked.push(write),
            Err(refusal) => return Err(refused(index, write, refusal)),
        }
    }
    Ok(checked)
}

/// Checks one write of `value` to `key` of `module_id`, timed or not,
/// against `directory`.
pub(crate) fn check_write(
    directory: &ControlSurfaceMap,
    module_id: &str,
    key: &str,
    value: &ControlValue,
    timed: bool,
) -> Result<CheckedWrite, WriteRefusal> {
    use ControlWriteReason::{InvalidControlValue, InvalidTiming, UnknownControl};
    let Some(surface) = directory.get(module_id) else {
        return Err(WriteRefusal::new(
            ControlWriteReason::UnknownModule,
            format!("no module '{module_id}' exists"),
        ));
    };
    let invalid = |message: String| WriteRefusal::new(InvalidControlValue, message);
    let setter = |reason: String| invalid(setter_refused(module_id, key, &reason));
    // A part of another control (a cell of a bank) is checked as the whole
    // it builds, and written at once.
    if let Some(whole) = surface.whole_write(key, value) {
        whole.map_err(setter)?;
        if timed {
            return Err(WriteRefusal::new(
                InvalidTiming,
                format!(
                    "control '{module_id}.{key}' is a part, written at once; it cannot be timed"
                ),
            ));
        }
        return Ok(CheckedWrite {
            value: value.clone(),
            clamped: false,
        });
    }
    // Its kind as listed, or as declared for one a count leaves unlisted
    // (a melody's `degree.6` while it has three), which a batch's earlier
    // write of the count may bring into play.
    let controls = surface.controls();
    let listed = controls.iter().find(|meta| meta.key == key);
    let kind = match listed {
        Some(meta) => meta.kind.clone(),
        None => match surface.declaration(key) {
            Some(declared) => declared.decl.listed_kind(),
            None => {
                let keys: Vec<String> = controls.into_iter().map(|meta| meta.key).collect();
                return Err(WriteRefusal::new(
                    UnknownControl,
                    unknown_control(module_id, key, &keys),
                ));
            }
        },
    };
    let exact = exact_number::keeps_text(value) && surface.is_whole(key);
    let coerced = match listed {
        Some(_) => surface.coerce_value(key, value.clone()),
        None if exact => value.clone(),
        None => value.clone().coerced_to(&kind),
    };
    if !exact && !fits_kind(&coerced, &kind) {
        return Err(invalid(wrong_kind(module_id, key, &kind, value)));
    }
    if let Some(message) = non_finite_refusal(module_id, key, &coerced) {
        return Err(invalid(message));
    }
    surface
        .validate_control(key, &coerced, directory)
        .map_err(setter)?;
    if timed && surface.writes_undeclared(key) {
        return Err(WriteRefusal::new(
            InvalidTiming,
            format!(
                "control '{module_id}.{key}' cannot be timed: it reaches undeclared host state, \
                 written at once"
            ),
        ));
    }
    let (value, clamped) = match &kind {
        ControlKind::Number { .. } => hold(coerced, surface.held_range(key)),
        ControlKind::String {
            options: Some(options),
            ..
        } => (as_offered(coerced, options), false),
        _ => (coerced, false),
    };
    Ok(CheckedWrite { value, clamped })
}

/// A choice written in another case than the option it names ("Sine" for
/// "sine"), as that option is spelled.
pub(crate) fn as_offered(value: ControlValue, options: &[String]) -> ControlValue {
    let ControlValue::String(text) = &value else {
        return value;
    };
    if options.iter().any(|option| option == text) {
        return value;
    }
    match (options.iter()).find(|option| option.eq_ignore_ascii_case(text.trim())) {
        Some(option) => ControlValue::String(option.clone()),
        None => value,
    }
}

/// `value`, a number control's write, held within `range`: the nearer limit
/// when it lies outside, and whether it did. Number text a whole-number
/// control keeps is read as the number it names.
pub(crate) fn hold(value: ControlValue, range: Option<(f32, f32)>) -> (ControlValue, bool) {
    let Some((least, most)) = range else {
        return (value, false);
    };
    let number = match &value {
        ControlValue::Number(number) => f64::from(*number),
        ControlValue::String(text) => match text.trim().parse::<f64>() {
            Ok(number) => number,
            Err(_) => return (value, false),
        },
        ControlValue::Bool(_) => return (value, false),
    };
    if number < f64::from(least) {
        (ControlValue::Number(least), true)
    } else if number > f64::from(most) {
        (ControlValue::Number(most), true)
    } else {
        (value, false)
    }
}

/// A write to a key `module_id` does not list among `keys`.
pub(crate) fn unknown_control(module_id: &str, key: &str, keys: &[String]) -> String {
    format!(
        "module '{module_id}' has no control '{key}' (available: {})",
        listing(keys)
    )
}

/// A write of `value`, not of `kind`, to `key` of `module_id`.
pub(crate) fn wrong_kind(
    module_id: &str,
    key: &str,
    kind: &ControlKind,
    value: &ControlValue,
) -> String {
    format!(
        "control '{module_id}.{key}' expects {}, got {}",
        kind_name(kind),
        echo(value)
    )
}

/// A write the module's own setter rules refuse, for `reason`.
pub(crate) fn setter_refused(module_id: &str, key: &str, reason: &str) -> String {
    format!(
        "module '{module_id}' refused the value for control '{key}': {}",
        bounded(reason, MODULE_ERROR_BYTES)
    )
}

/// Whether a coerced value has the shape its control declares. Option
/// lists are left to the module, which may accept aliases: its setter
/// rules (`validate_control`) refuse what its setter would.
pub(crate) fn fits_kind(value: &ControlValue, kind: &ControlKind) -> bool {
    matches!(
        (value, kind),
        (ControlValue::Number(_), ControlKind::Number { .. })
            | (ControlValue::Bool(_), ControlKind::Bool)
            | (ControlValue::String(_), ControlKind::String { .. })
    )
}

fn kind_name(kind: &ControlKind) -> &'static str {
    match kind {
        ControlKind::Number { .. } => "a number",
        ControlKind::Bool => "a boolean",
        ControlKind::String { .. } => "a string",
    }
}

/// A refused value as a message shows it, cut to about
/// [`ECHOED_VALUE_BYTES`].
fn echo(value: &ControlValue) -> String {
    match value {
        ControlValue::Number(number) => number.to_string(),
        ControlValue::Bool(flag) => flag.to_string(),
        ControlValue::String(text) => format!("{:?}", bounded(text, ECHOED_VALUE_BYTES)),
    }
}

#[cfg(test)]
#[path = "write_rule_tests.rs"]
mod tests;
