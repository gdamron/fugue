//! Timed structural edits in the session log: what each changed, when it
//! was timed for, and whether it landed (on which sample) or was refused,
//! so a replay lands it on the same sample or leaves it out as live did.
//!
//! An edit is recorded as the graph it leaves rather than the request that
//! made it (a swap, an edit batch, a reload): the modules it removed, those
//! it built (rebuilt in place, or added after the rest, in order) and every
//! connection afterwards, in order. A replay makes the same change through
//! the same path an offline edit takes, so schedulers re-resolve and
//! survivors keep their state as they did live. Its writes to modules it
//! kept are logged as requests are.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::LoggedBeat;
use crate::control_request::{Outcome, Refusal};
use crate::invention::graph::RoutingConnection;
use crate::invention::publish::BuiltModule;

/// When a timed edit was timed for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LoggedWhen {
    /// On this sample.
    AtSample { sample: u64 },
    /// This many samples after it was queued.
    AfterSamples { samples: u64 },
    /// On a clock's beat.
    Beat { beat: LoggedBeat },
    /// Once a condition was met.
    Condition,
}

/// What became of a timed edit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LoggedEditOutcome {
    /// It landed on `sample`, after every request applied at or before it;
    /// `due` is the sample it was due at, when it landed late.
    Landed {
        sample: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        due: Option<u64>,
    },
    /// It never applied: `reason` says why (the audio thread's refusal, in
    /// snake case, such as `target_gone` or `expired`).
    Refused { reason: String },
}

/// A module a timed edit built.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct LoggedModule {
    pub id: String,
    pub module_type: String,
    pub config: serde_json::Value,
    /// The control writes made through its setters once built, in order,
    /// before it was installed (an edit batch's to a module it builds).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writes: Vec<LoggedSetting>,
}

/// A control write made to a module a timed edit built, before it landed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct LoggedSetting {
    pub key: String,
    pub value: crate::ControlValue,
}

/// A connection, as a timed edit left it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct LoggedConnection {
    pub from: String,
    pub from_port: String,
    pub to: String,
    pub to_port: String,
}

/// One timed structural edit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct LoggedEdit {
    pub when: LoggedWhen,
    pub outcome: LoggedEditOutcome,
    /// The modules it removed.
    #[serde(default)]
    pub removed: Vec<String>,
    /// The modules it built, in the order the graph holds them after it.
    #[serde(default)]
    pub built: Vec<LoggedModule>,
    /// Every connection after it, in order.
    #[serde(default)]
    pub connections: Vec<LoggedConnection>,
    /// How many logged requests applied before the audio thread took it,
    /// so a replay latches its clock (one taken on its beat) where live
    /// did: before the requests after it, such as its own tempo write.
    #[serde(default)]
    pub requests_before: usize,
}

impl LoggedEdit {
    /// The change from the graph `before` (its modules in order) to the
    /// one `after` holds with its `edges`, building `built`, as queued at
    /// `when`, its outcome yet to come. `None` when a replay making it
    /// would not leave the modules in the same order (one it rebuilt was
    /// removed and added again, after the rest): it is then counted as an
    /// edit the log cannot replay.
    pub(crate) fn queued<B, A>(
        when: LoggedWhen,
        before: &IndexMap<String, B>,
        (after, edges): (&IndexMap<String, A>, &[RoutingConnection]),
        built: &IndexMap<String, BuiltModule>,
    ) -> Option<Self> {
        let removed: Vec<String> = before
            .keys()
            .filter(|id| !after.contains_key(*id))
            .cloned()
            .collect();
        let built: Vec<LoggedModule> = after
            .keys()
            .filter_map(|id| built.get(id))
            .map(|module| LoggedModule {
                id: module.info.id.clone(),
                module_type: module.info.module_type.clone(),
                config: module.info.config.clone(),
                writes: (module.writes.iter())
                    .map(|(key, value)| LoggedSetting {
                        key: key.clone(),
                        value: value.clone(),
                    })
                    .collect(),
            })
            .collect();
        // What a replay leaves: those kept in place, then those added.
        let mut order: Vec<&String> = before.keys().filter(|id| after.contains_key(*id)).collect();
        order.extend(
            built
                .iter()
                .map(|m| &m.id)
                .filter(|id| !before.contains_key(*id)),
        );
        if !order.into_iter().eq(after.keys()) {
            return None;
        }
        let connections = edges
            .iter()
            .map(|edge| LoggedConnection {
                from: edge.from_module.clone(),
                from_port: edge.from_port.clone(),
                to: edge.to_module.clone(),
                to_port: edge.to_port.clone(),
            })
            .collect();
        Some(Self {
            when,
            // Until its outcome comes.
            outcome: LoggedEditOutcome::Refused {
                reason: "pending".to_string(),
            },
            removed,
            built,
            connections,
            requests_before: 0,
        })
    }

    /// This edit settled by `outcome`.
    pub(crate) fn settled(mut self, outcome: Outcome) -> Self {
        self.outcome = match outcome {
            Outcome::Applied { at } => LoggedEditOutcome::Landed {
                sample: at,
                due: None,
            },
            Outcome::AppliedLate { at, due } => LoggedEditOutcome::Landed {
                sample: at,
                due: Some(due),
            },
            Outcome::Superseded => refused("superseded"),
            Outcome::Overridden => refused("overridden"),
            Outcome::Refused(refusal) => refused(&snake_case(refusal)),
        };
        self
    }

    /// The sample it landed on, if it did.
    pub fn landed(&self) -> Option<u64> {
        match self.outcome {
            LoggedEditOutcome::Landed { sample, .. } => Some(sample),
            LoggedEditOutcome::Refused { .. } => None,
        }
    }
}

fn refused(reason: &str) -> LoggedEditOutcome {
    LoggedEditOutcome::Refused {
        reason: reason.to_string(),
    }
}

/// `TargetGone` as `target_gone`.
fn snake_case(refusal: Refusal) -> String {
    let mut name = String::new();
    for (i, c) in format!("{refusal:?}").chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            name.push('_');
        }
        name.push(c.to_ascii_lowercase());
    }
    name
}
