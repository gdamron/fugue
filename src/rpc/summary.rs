//! A small, slow picture of the session for surfaces that can't follow the
//! live channel: what is loaded, where the music is, the levels and whether
//! the session is saved.
//!
//! A daemon refreshes it about once a second from state it already keeps on
//! the control side, so reading it costs the audio thread nothing. The shape
//! carries its own [`StatusSummary::version`] and stays under
//! [`MAX_STATUS_SUMMARY_BYTES`], so a relay can pass it on unchanged. New
//! fields arrive as optional additions within a version; a reader ignores
//! fields it doesn't know.

use serde::{Deserialize, Serialize};

use super::{FocusBars, RuntimeRevision, SessionDurability};
use crate::ControlValue;

/// The summary shape this build writes.
pub const STATUS_SUMMARY_VERSION: u32 = 1;
/// Largest a summary serializes to, as JSON.
pub const MAX_STATUS_SUMMARY_BYTES: usize = 8 * 1024;
/// Most modules a summary names; `module_count` still counts them all.
pub const MAX_SUMMARY_MODULES: usize = 16;
/// Most clocks a summary reports.
pub const MAX_SUMMARY_CLOCKS: usize = 2;
/// Most pinned controls a summary carries.
pub const MAX_SUMMARY_PINNED: usize = 16;
/// Longest name, id or text value a summary carries, in bytes; longer ones
/// are cut at a character boundary.
pub const MAX_SUMMARY_TEXT_BYTES: usize = 64;
/// Longest save error a summary carries, in bytes.
const MAX_SUMMARY_ERROR_BYTES: usize = 256;

/// The session at a glance, as last published.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct StatusSummary {
    /// The shape's version: [`STATUS_SUMMARY_VERSION`] when written.
    pub version: u32,
    /// Counts publications, from 1, within one daemon session: a reader that
    /// sees the same `seq` again from the same `revision.session_id` has
    /// nothing new. A restarted daemon counts from 1 again.
    pub seq: u64,
    /// When it was published, in milliseconds since the Unix epoch.
    pub published_at_ms: u64,
    /// The authoring revision it describes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<RuntimeRevision>,
    /// What is loaded; none when nothing is.
    pub loaded: Option<LoadedSummary>,
    pub transport: TransportSummary,
    /// The controls pinned for the session, with their values. None where
    /// the daemon has no pinning yet, as against an empty list when nothing
    /// is pinned.
    pub pinned: Option<Vec<PinnedControl>>,
    /// The output's latest levels; none while nothing plays.
    pub levels: Option<OutputLevels>,
    /// Where saving the session stands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionDurability>,
    /// What the person last pointed at, in brief.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<FocusSummary>,
}

/// The invention that is loaded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct LoadedSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The file name it was loaded from, without its directory: a summary
    /// may leave the machine, a local path should not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    pub module_count: usize,
    pub connection_count: usize,
    /// The first [`MAX_SUMMARY_MODULES`] modules, in document order.
    pub modules: Vec<SummaryModule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct SummaryModule {
    pub id: String,
    #[serde(rename = "type")]
    pub module_type: String,
    /// What the piece calls the module, when it names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Whether the music plays, and where it is.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct TransportSummary {
    /// Whether the music plays: false with nothing loaded and while a
    /// loaded piece is paused ([`RuntimeStatus::playing`]).
    ///
    /// [`RuntimeStatus::playing`]: crate::RuntimeStatus::playing
    pub playing: bool,
    /// The invention's clocks, in document order, up to
    /// [`MAX_SUMMARY_CLOCKS`].
    pub clocks: Vec<ClockSummary>,
}

/// A clock's tempo and musical position.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ClockSummary {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f32>,
    /// Beats counted since the clock started, across resets: its `elapsed`
    /// control, the measure a view's playhead uses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beats: Option<f64>,
    /// The bar, from 1 at the clock's latest reset; none without a meter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bar: Option<u32>,
    /// The beat within the bar, from 1; none without a meter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beat_in_bar: Option<u32>,
    /// Beats in the bar playing now: the meter in force (a new one takes
    /// over at the next bar line); none without a meter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beats_per_bar: Option<u32>,
}

impl ClockSummary {
    /// Clock `id`'s summary from its controls, as `control` reads them by
    /// key (a running invention's `get_control`, say): its `bpm`, its
    /// `elapsed` beats to the thousandth, and with a meter in force its
    /// `bar`, `beat_in_bar` and `meter`. A control it cannot read is left
    /// out.
    pub fn read(id: &str, control: impl Fn(&str) -> Option<ControlValue>) -> Self {
        let number = |key: &str| match control(key) {
            Some(ControlValue::Number(value)) if value.is_finite() => Some(value),
            _ => None,
        };
        // 0 is no meter, and a count from 1.
        let count = |key: &str| number(key).filter(|n| *n >= 1.0).map(|n| n as u32);
        let beats_per_bar = count("meter");
        let metered = |key: &str| beats_per_bar.and(count(key));
        Self {
            id: id.to_string(),
            bpm: number("bpm"),
            // To the thousandth: an f32 widened to f64 prints noise digits.
            beats: number("elapsed").map(|beats| (f64::from(beats) * 1e3).round() / 1e3),
            bar: metered("bar"),
            beat_in_bar: metered("beat_in_bar"),
            beats_per_bar,
        }
    }
}

/// A pinned control and its current value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct PinnedControl {
    pub module_id: String,
    pub key: String,
    pub value: ControlValue,
}

/// The output's levels over the latest meter window, linear 0..1 (above 1
/// clips).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct OutputLevels {
    pub left_peak: f32,
    pub right_peak: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_rms: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_rms: Option<f32>,
    /// Non-finite samples the output's sinks have replaced with silence: the
    /// same running total as `MeterReading::non_finite_samples`. Left out on
    /// the wire while it is zero.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub non_finite_samples: u64,
}

fn is_zero(count: &u64) -> bool {
    *count == 0
}

/// The session's focus without its notes or nodes: where the person pointed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct FocusSummary {
    pub view: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_id: Option<String>,
    /// The selection's kind: `notes`, `time-range`, `voice` or `graph`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bars: Option<FocusBars>,
    pub selected_at_ms: u64,
}

impl StatusSummary {
    /// Cuts every name and list to the summary's limits, then drops named
    /// modules and pinned controls from the end until the whole serializes
    /// within [`MAX_STATUS_SUMMARY_BYTES`]. Counts are kept, so a reader
    /// still knows how many there are.
    pub fn bounded(mut self) -> Self {
        if let Some(loaded) = self.loaded.as_mut() {
            cut_opt(&mut loaded.title);
            cut_opt(&mut loaded.file);
            loaded.modules.truncate(MAX_SUMMARY_MODULES);
            for module in &mut loaded.modules {
                cut(&mut module.id);
                cut(&mut module.module_type);
                cut_opt(&mut module.name);
            }
        }
        self.transport.clocks.truncate(MAX_SUMMARY_CLOCKS);
        for clock in &mut self.transport.clocks {
            cut(&mut clock.id);
        }
        if let Some(pinned) = self.pinned.as_mut() {
            pinned.truncate(MAX_SUMMARY_PINNED);
            for control in pinned {
                cut(&mut control.module_id);
                cut(&mut control.key);
                if let ControlValue::String(text) = &mut control.value {
                    cut(text);
                }
            }
        }
        if let Some(error) = self.session.as_mut().and_then(|s| s.error.as_mut()) {
            cut_to(error, MAX_SUMMARY_ERROR_BYTES);
        }
        if let Some(focus) = self.focus.as_mut() {
            cut(&mut focus.view);
            cut_opt(&mut focus.module_id);
            cut(&mut focus.kind);
        }
        // Escaping can still lengthen what was cut; give up names, never
        // the limit.
        while self.serialized_len() > MAX_STATUS_SUMMARY_BYTES {
            let dropped = match self.loaded.as_mut() {
                Some(loaded) if !loaded.modules.is_empty() => loaded.modules.pop().is_some(),
                _ => self
                    .pinned
                    .as_mut()
                    .and_then(|pinned| pinned.pop())
                    .is_some(),
            };
            if !dropped {
                break;
            }
        }
        self
    }

    /// Its size as JSON, in bytes.
    pub fn serialized_len(&self) -> usize {
        serde_json::to_vec(self).map_or(usize::MAX, |bytes| bytes.len())
    }
}

fn cut(text: &mut String) {
    cut_to(text, MAX_SUMMARY_TEXT_BYTES);
}

fn cut_opt(text: &mut Option<String>) {
    if let Some(text) = text.as_mut() {
        cut(text);
    }
}

fn cut_to(text: &mut String, max: usize) {
    if text.len() <= max {
        return;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}
