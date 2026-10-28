//! A guest's failure, signalled by the audio thread and read on a control
//! thread.
//!
//! A guest that traps or returns an error latches its module muted (see
//! `WasmModule::fail`). The audio thread must not print that: formatting
//! allocates and the standard error stream takes a lock. It moves the error
//! into a [`OnceLock`] instead, as the call returned it, and a control
//! thread formats it later through a [`WasmModuleHandle`].

use std::sync::{Arc, OnceLock};

/// The error a guest call ended with, as the call returned it.
pub(super) enum Reason {
    /// The guest returned `err(message)`.
    Guest(String),
    /// The call trapped, or the host could not lower or lift its values.
    Trap(wasmtime::Error),
}

struct Failure {
    export: &'static str,
    reason: Reason,
}

/// What the module and its handles share.
#[derive(Default)]
struct Shared {
    /// Set when the module loads, on a control thread.
    name: OnceLock<String>,
    /// Set by the first failure, on the audio thread: lock-free and
    /// allocation-free, because only the audio thread writes it and
    /// readers only look.
    failure: OnceLock<Failure>,
}

/// The module's side of the signal.
#[derive(Clone, Default)]
pub(super) struct FailureSignal(Arc<Shared>);

impl FailureSignal {
    /// Names the module in the message a control thread formats. Control
    /// thread, once.
    pub(super) fn name(&self, name: &str) {
        let _ = self.0.name.set(name.to_string());
    }

    /// Records that `export` failed with `reason`. Never locks or
    /// allocates, so the audio thread may call it; only the first failure
    /// is kept, and a latched module fails once.
    pub(super) fn record(&self, export: &'static str, reason: Reason) {
        let _ = self.0.failure.set(Failure { export, reason });
    }

    /// The message for a failure, if one was recorded. Allocates: control
    /// threads only.
    pub(super) fn message(&self) -> Option<String> {
        let failure = self.0.failure.get()?;
        let name = self.0.name.get().map_or("", String::as_str);
        let export = failure.export;
        // `{error:#}` prints wasmtime's whole context chain, whose outermost
        // link is only the wasm backtrace.
        Some(match &failure.reason {
            Reason::Guest(error) => format_failure(name, export, error),
            Reason::Trap(error) => format_failure(name, export, &format!("{error:#}")),
        })
    }
}

fn format_failure(name: &str, export: &str, error: &str) -> String {
    format!("wasm module '{name}' failed in `{export}`: {error}; muting output")
}

/// Reads whether a `wasm_module` has failed, from any thread but the audio
/// thread. The module's `handle` build handle holds one.
#[derive(Clone)]
pub struct WasmModuleHandle(FailureSignal);

impl WasmModuleHandle {
    pub(super) fn new(signal: FailureSignal) -> Self {
        Self(signal)
    }

    /// Why the guest stopped, if it did: a trap or an error from one of its
    /// exports. The module is muted from then on. Allocates a message, so
    /// call it on a control thread.
    pub fn failure(&self) -> Option<String> {
        self.0.message()
    }
}
