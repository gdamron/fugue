//! The session's pinned controls: the controls that matter for this piece,
//! kept in view across a session.
//!
//! An agent pins the controls it shows the person, and either of them can
//! pin or unpin one by name. Every surface reads the same set, so the MCP
//! App, a persistent view and the web app all show it. A pin lasts while its
//! module and control run: a reload or an edit that removes one drops its
//! pin, and the daemon says which it dropped. Pins change no music, so
//! editing them moves no authoring revision; a daemon saves them with the
//! session.

use serde::{Deserialize, Serialize};

use super::{RequestSource, RpcError, RpcErrorCode, MAX_SUMMARY_PINNED};
use crate::ControlValue;

/// Most controls a session pins. The status summary carries them all.
pub const MAX_PINNED_CONTROLS: usize = MAX_SUMMARY_PINNED;
/// Longest module id or control key a pin names, in bytes.
pub const MAX_PIN_NAME_BYTES: usize = 256;

/// A control, named as `set_control` names it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ControlAddress {
    pub module_id: String,
    pub key: String,
}

/// One pinned control, and who pinned it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ControlPin {
    pub module_id: String,
    pub key: String,
    /// Who pinned it: the person (`user`) or an agent.
    #[serde(default)]
    pub added_by: RequestSource,
    /// Its current value, where the reply reads it; none in an event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<ControlValue>,
}

/// Pins and unpins controls in one step: unpinned first, then pinned.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct PinEdit {
    /// Controls to pin, after those already pinned. One already pinned
    /// keeps its place and who pinned it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pin: Vec<ControlAddress>,
    /// Controls to unpin. One not pinned is ignored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unpin: Vec<ControlAddress>,
    /// Who makes the edit; a user when omitted.
    #[serde(default)]
    pub source: RequestSource,
}

/// The pinned set, in the order pinned.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct PinList {
    pub pins: Vec<ControlPin>,
    /// Pins the latest change to the running graph dropped because their
    /// module or control is gone. Kept until the next such drop or edit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped: Vec<ControlAddress>,
}

impl ControlPin {
    pub fn address(&self) -> ControlAddress {
        ControlAddress {
            module_id: self.module_id.clone(),
            key: self.key.clone(),
        }
    }

    fn is(&self, address: &ControlAddress) -> bool {
        self.module_id == address.module_id && self.key == address.key
    }
}

impl PinEdit {
    /// Applies this edit to the session's `pins`. `exists` says whether a
    /// module runs with a control of that key: pinning one that does not is
    /// refused. So is an edit that would leave more than
    /// [`MAX_PINNED_CONTROLS`] pinned. A refused edit changes nothing.
    pub fn apply(
        self,
        pins: &mut Vec<ControlPin>,
        exists: impl Fn(&str, &str) -> bool,
    ) -> Result<(), RpcError> {
        for address in self.pin.iter().chain(&self.unpin) {
            for (what, name) in [("module_id", &address.module_id), ("key", &address.key)] {
                if name.len() > MAX_PIN_NAME_BYTES {
                    return Err(invalid(format!(
                        "{what} is longer than {MAX_PIN_NAME_BYTES} bytes"
                    )));
                }
            }
        }
        let missing: Vec<String> = (self.pin.iter())
            .filter(|a| !exists(&a.module_id, &a.key))
            .map(|a| format!("{}.{}", a.module_id, a.key))
            .collect();
        if !missing.is_empty() {
            return Err(RpcError::new(
                RpcErrorCode::UnknownModule,
                format!("no running control to pin: {}", missing.join(", ")),
            ));
        }
        let mut next: Vec<ControlPin> = pins
            .iter()
            .filter(|pin| !self.unpin.iter().any(|a| pin.is(a)))
            .cloned()
            .collect();
        for address in self.pin {
            if !next.iter().any(|pin| pin.is(&address)) {
                next.push(ControlPin {
                    module_id: address.module_id,
                    key: address.key,
                    added_by: self.source,
                    value: None,
                });
            }
        }
        if next.len() > MAX_PINNED_CONTROLS {
            return Err(invalid(format!(
                "at most {MAX_PINNED_CONTROLS} controls stay pinned, and this would leave {}; unpin some first",
                next.len()
            )));
        }
        *pins = next;
        Ok(())
    }
}

/// Keeps the pins whose control `exists` says still runs, returning the
/// ones it dropped, in order.
pub fn retain_pins(
    pins: &mut Vec<ControlPin>,
    exists: impl Fn(&str, &str) -> bool,
) -> Vec<ControlAddress> {
    let mut dropped = Vec::new();
    pins.retain(|pin| {
        let keep = exists(&pin.module_id, &pin.key);
        if !keep {
            dropped.push(pin.address());
        }
        keep
    });
    dropped
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(RpcErrorCode::InvalidRequest, message)
}
