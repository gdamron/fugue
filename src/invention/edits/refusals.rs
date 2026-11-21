//! How an edit batch words the refusal of one structural edit.
//!
//! The daemon's `ApplyEdits` and an offline render's batch (see
//! [`RenderEngine::apply_edits`](crate::invention::render::RenderEngine::apply_edits))
//! check the same edits in the same order, and refuse each the same way
//! through these, so an agent reads the same reason and message from
//! either.

use crate::rpc::{truncate_on_char_boundary, EditFailureReason, MODULE_ERROR_BYTES};

/// How many names a refusal lists when it shows what is available.
const LISTED_NAMES: usize = 24;

/// Why an edit cannot apply; the caller adds its index and op.
#[derive(Debug)]
pub(crate) struct Refusal(pub(crate) EditFailureReason, pub(crate) String);

pub(crate) fn unknown_module(id: &str) -> Refusal {
    Refusal(
        EditFailureReason::UnknownModule,
        format!("no module '{id}' exists at this point in the batch"),
    )
}

pub(crate) fn duplicate_module(id: &str) -> Refusal {
    Refusal(
        EditFailureReason::DuplicateModule,
        format!("module '{id}' already exists; remove it first to replace it"),
    )
}

pub(crate) fn unknown_module_type(module_type: &str) -> Refusal {
    Refusal(
        EditFailureReason::UnknownModuleType,
        format!("unknown module type '{module_type}'"),
    )
}

/// A module whose factory refused `config`, with the factory's reason cut
/// to about [`MODULE_ERROR_BYTES`].
pub(crate) fn refused_config(id: &str, module_type: &str, error: &str) -> Refusal {
    Refusal(
        EditFailureReason::InvalidConfig,
        format!(
            "module '{id}' of type '{module_type}' refused the config: {}",
            bounded(error, MODULE_ERROR_BYTES)
        ),
    )
}

pub(crate) fn no_output_port(id: &str, port: &str, outputs: &[String]) -> Refusal {
    Refusal(
        EditFailureReason::UnknownPort,
        format!(
            "module '{id}' has no output port '{port}' (available: {})",
            listing(outputs)
        ),
    )
}

pub(crate) fn no_input_port(id: &str, port: &str, inputs: &[String]) -> Refusal {
    Refusal(
        EditFailureReason::UnknownPort,
        format!(
            "module '{id}' has no input port '{port}' (available: {})",
            listing(inputs)
        ),
    )
}

pub(crate) fn already_connected(from: &str, from_port: &str, to: &str, to_port: &str) -> Refusal {
    Refusal(
        EditFailureReason::ConnectionExists,
        format!("{from}.{from_port} is already connected to {to}.{to_port}"),
    )
}

pub(crate) fn not_connected(from: &str, from_port: &str, to: &str, to_port: &str) -> Refusal {
    Refusal(
        EditFailureReason::ConnectionNotFound,
        format!("{from}.{from_port} is not connected to {to}.{to_port}"),
    )
}

/// `text` cut at a character boundary to at most `max` bytes, marked with
/// `…` when cut.
pub(crate) fn bounded(text: &str, max: usize) -> String {
    let mut cut = text.to_string();
    truncate_on_char_boundary(&mut cut, max);
    if cut.len() < text.len() {
        cut.push('…');
    }
    cut
}

pub(crate) fn listing(names: &[String]) -> String {
    if names.is_empty() {
        return "none".to_string();
    }
    let shown = names.iter().take(LISTED_NAMES).cloned().collect::<Vec<_>>();
    let more = names.len().saturating_sub(LISTED_NAMES);
    if more == 0 {
        shown.join(", ")
    } else {
        format!("{}, and {more} more", shown.join(", "))
    }
}
