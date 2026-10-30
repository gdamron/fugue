//! The session's focus: what the person last pointed at in a view.
//!
//! A view (a piano roll, the graph, a waveform) reports a selection as a
//! [`FocusIntent`]; the daemon keeps the latest as the session's [`Focus`],
//! whichever surface it came from, and an agent reads it to learn what "this"
//! means in "make this busier". Pointing never changes the music.

use serde::{Deserialize, Serialize};

use super::{RpcError, RpcErrorCode};

/// Most notes, graph nodes or connections one focus names.
pub const MAX_FOCUS_ITEMS: usize = 256;
/// Longest view name, module id, voice or port name a focus carries.
pub const MAX_FOCUS_NAME_BYTES: usize = 256;

/// A view's selection, or its clearing, as the view reports it.
///
/// The fields match the views' `fyug-select` event detail, so a host passes
/// the detail on unchanged. Without a `selection` it is the view's
/// `fyug-select-clear`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct FocusIntent {
    /// The kind of view the selection was made in, such as `piano-roll`.
    pub view: String,
    /// The module the view shows; none for a view of the whole invention.
    #[serde(default)]
    pub module_id: Option<String>,
    /// What was selected; none clears this view's selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<FocusTarget>,
    /// The view's bar length in beats, so beat times can be read as bars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beats_per_bar: Option<f64>,
}

/// What a selection points at.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FocusTarget {
    /// Particular notes, named by what was heard.
    Notes {
        unit: FocusTimeUnit,
        notes: Vec<FocusNote>,
    },
    /// A stretch of time, such as bars 3-4 or a span of a waveform.
    TimeRange {
        unit: FocusTimeUnit,
        start: f64,
        end: f64,
    },
    /// One voice of the module, all its notes.
    Voice { voice: String },
    /// Modules (by id) and connections in the invention graph.
    Graph {
        #[serde(default)]
        nodes: Vec<String>,
        #[serde(default)]
        connections: Vec<FocusConnection>,
    },
}

/// The unit of a selection's times: clock beats, or seconds where a view
/// has no beats.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FocusTimeUnit {
    Beats,
    Seconds,
}

/// A note, named by its voice, pitch and time: generated notes have no other
/// identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct FocusNote {
    pub voice: String,
    /// MIDI note number.
    pub pitch: u8,
    pub start: f64,
    /// None while the note is still held.
    #[serde(default)]
    pub end: Option<f64>,
}

/// A connection, as the invention document names it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct FocusConnection {
    pub from: String,
    pub from_port: String,
    pub to: String,
    pub to_port: String,
}

/// Bars a selection covers, numbered from 1, both inclusive.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct FocusBars {
    pub first: u64,
    pub last: u64,
}

/// The session's focus: the latest selection on any surface.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct Focus {
    /// The kind of view it was made in.
    pub view: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_id: Option<String>,
    pub selection: FocusTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beats_per_bar: Option<f64>,
    /// The bars the selection covers, when its times are in beats and the
    /// view gave its bar length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bars: Option<FocusBars>,
    /// When it was selected, in milliseconds since the Unix epoch.
    pub selected_at_ms: u64,
}

impl FocusIntent {
    /// Applies this selection, or clearing, to the session's `focus`.
    ///
    /// A selection replaces the focus whichever view made it. A clearing
    /// drops it only when the same view of the same module made it, so a view
    /// letting go never erases a newer selection elsewhere. `exists` says
    /// whether a module is in the running invention: a selection naming one
    /// that is not is refused, and nothing changes.
    pub fn apply(
        self,
        focus: &mut Option<Focus>,
        selected_at_ms: u64,
        exists: impl Fn(&str) -> bool,
    ) -> Result<(), RpcError> {
        self.check(&exists)?;
        let Some(selection) = self.selection else {
            let same_view = focus
                .as_ref()
                .is_some_and(|f| f.view == self.view && f.module_id == self.module_id);
            if same_view {
                *focus = None;
            }
            return Ok(());
        };
        let bars = self.beats_per_bar.and_then(|bpb| selection.bars(bpb));
        *focus = Some(Focus {
            view: self.view,
            module_id: self.module_id,
            selection,
            beats_per_bar: self.beats_per_bar,
            bars,
            selected_at_ms,
        });
        Ok(())
    }

    fn check(&self, exists: &impl Fn(&str) -> bool) -> Result<(), RpcError> {
        name("view", &self.view)?;
        if self.view.is_empty() {
            return Err(invalid("view must name the view the selection was made in"));
        }
        if let Some(bpb) = self.beats_per_bar {
            if !(bpb.is_finite() && bpb > 0.0) {
                return Err(invalid("beats_per_bar must be a positive number"));
            }
        }
        if let Some(module) = &self.module_id {
            name("module_id", module)?;
            known(module, exists)?;
        }
        // A clearing names only the view; there is nothing more to check.
        match &self.selection {
            Some(selection) => selection.check(exists),
            None => Ok(()),
        }
    }
}

impl FocusTarget {
    fn check(&self, exists: &impl Fn(&str) -> bool) -> Result<(), RpcError> {
        match self {
            Self::Notes { notes, .. } => {
                items("notes", notes.len())?;
                for note in notes {
                    name("voice", &note.voice)?;
                    span(note.start, note.end.unwrap_or(note.start))?;
                }
                Ok(())
            }
            Self::TimeRange { start, end, .. } => span(*start, *end),
            Self::Voice { voice } => name("voice", voice),
            Self::Graph { nodes, connections } => {
                items("nodes", nodes.len())?;
                items("connections", connections.len())?;
                if nodes.is_empty() && connections.is_empty() {
                    return Err(invalid("a graph selection names at least one node"));
                }
                for node in nodes {
                    name("node", node)?;
                    known(node, exists)?;
                }
                for c in connections {
                    for port in [&c.from_port, &c.to_port] {
                        name("port", port)?;
                    }
                    for module in [&c.from, &c.to] {
                        name("module", module)?;
                        known(module, exists)?;
                    }
                }
                Ok(())
            }
        }
    }

    /// The bars this selection's beat times cover, for bars `beats_per_bar`
    /// long. None for a selection without beat times.
    fn bars(&self, beats_per_bar: f64) -> Option<FocusBars> {
        let (start, end) = match self {
            Self::Notes {
                unit: FocusTimeUnit::Beats,
                notes,
            } => notes.iter().fold(None, |range, n| {
                let end = n.end.unwrap_or(n.start);
                Some(match range {
                    None => (n.start, end),
                    Some((s, e)) => (f64::min(s, n.start), f64::max(e, end)),
                })
            })?,
            Self::TimeRange {
                unit: FocusTimeUnit::Beats,
                start,
                end,
            } => (*start, *end),
            _ => return None,
        };
        // A bar's last beat ends where the next bar starts, so an end on a
        // bar line belongs to the bar before it.
        let first = (start / beats_per_bar + 1e-9).floor();
        let last = ((end / beats_per_bar - 1e-9).ceil() - 1.0).max(first);
        Some(FocusBars {
            first: first as u64 + 1,
            last: last as u64 + 1,
        })
    }
}

impl Focus {
    /// Keeps the focus to modules `exists` says are still in the invention.
    /// Returns false when nothing it named is left, so it should be cleared.
    pub fn retain_modules(&mut self, exists: impl Fn(&str) -> bool) -> bool {
        if self.module_id.as_deref().is_some_and(|id| !exists(id)) {
            return false;
        }
        match &mut self.selection {
            FocusTarget::Graph { nodes, connections } => {
                nodes.retain(|id| exists(id));
                connections.retain(|c| exists(&c.from) && exists(&c.to));
                !(nodes.is_empty() && connections.is_empty())
            }
            _ => true,
        }
    }
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(RpcErrorCode::InvalidRequest, message)
}

fn name(what: &str, value: &str) -> Result<(), RpcError> {
    if value.len() > MAX_FOCUS_NAME_BYTES {
        return Err(invalid(format!(
            "{what} is longer than {MAX_FOCUS_NAME_BYTES} bytes"
        )));
    }
    Ok(())
}

fn items(what: &str, count: usize) -> Result<(), RpcError> {
    if count > MAX_FOCUS_ITEMS {
        return Err(invalid(format!(
            "a selection names at most {MAX_FOCUS_ITEMS} {what}, got {count}"
        )));
    }
    Ok(())
}

fn span(start: f64, end: f64) -> Result<(), RpcError> {
    if !(start.is_finite() && end.is_finite() && start >= 0.0 && end >= start) {
        return Err(invalid(format!(
            "a selected time must run forward from 0 or later, got {start} to {end}"
        )));
    }
    Ok(())
}

fn known(module: &str, exists: &impl Fn(&str) -> bool) -> Result<(), RpcError> {
    if exists(module) {
        Ok(())
    } else {
        Err(RpcError::new(
            RpcErrorCode::UnknownModule,
            format!("no module '{module}' is running to select"),
        ))
    }
}
