//! The notes a running invention plays, for views: a piano roll following
//! a sequencer, say.
//!
//! The audio thread taps every top-level module that plays notes (see
//! `graph::notes`) and sends each gate crossing through a ring. A control
//! thread drains it with [`RunningInvention::take_notes`], which pairs each
//! voice's starts and ends into [`NoteChange`]s: one as a note starts,
//! still held, and one as it ends. A voice plays one note at a time, so a
//! start on a voice still holding one ends that one first.
//!
//! Times are samples on the audio clock and, when the module has a clock
//! (its timeline context, see `crate::invention::timelines`), beats on that
//! clock counted across its resets: what its `elapsed` control reports, so
//! a view's playhead and its notes agree.
//!
//! [`RunningInvention::take_notes`]: crate::RunningInvention::take_notes

use std::collections::HashMap;
use std::sync::Arc;

use super::graph::notes::{NoteEdge, NoteOut, NoteTap, NoteTaps, TapVoice, NOTE_EDGE_CAPACITY};
use super::publish::TopologyMirror;
use super::timelines::Context;
use crate::audio_thread::ControlLock;
use crate::control_request::{EventCounter, EventCursor};
use crate::modules::divisi::MAX_VOICES;
use crate::spsc::Ring;

/// A note starting, or ending, on one voice of a module.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteChange {
    /// The module playing it.
    pub module_id: String,
    /// Numbers the module's notes from 0, in the order they start, for as
    /// long as the invention runs: the start and the end of one note carry
    /// the same number.
    pub note: u64,
    /// The voice: 0 for a module's lone `gate`, `N` for its `gate.N`.
    pub voice: u32,
    /// MIDI pitch (60 is middle C), fractional when the frequency falls
    /// between notes.
    pub pitch: f32,
    /// How hard it was struck, 0 to 1: the module's `velocity` output as it
    /// started, or 1 without one.
    pub velocity: f32,
    /// The clock its beats count on; `None` without one. A note is on one
    /// clock from start to end: one held as its module moves to another
    /// clock ends, and starts again on the new one.
    pub clock: Option<String>,
    pub start: NoteTime,
    /// `None` while it is held.
    pub end: Option<NoteTime>,
}

/// When a note starts or ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteTime {
    /// The sample, counted from zero as the invention started playing.
    pub frame: u64,
    /// The beat on the module's clock, counted across the clock's resets;
    /// `None` when the module has no clock.
    pub beats: Option<f64>,
}

/// Where a module's notes are now, for a view's playhead.
#[derive(Clone, Debug, PartialEq)]
pub struct Playhead {
    /// The clock its notes count beats on.
    pub clock: String,
    /// That clock's beats counted across its resets, as of the latest
    /// block: the measure [`NoteTime::beats`] uses.
    pub beats: f64,
}

/// What [`RunningInvention::take_notes`] drained.
///
/// [`RunningInvention::take_notes`]: crate::RunningInvention::take_notes
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoteChanges {
    /// In the order they happened.
    pub notes: Vec<NoteChange>,
    /// Starts and ends the audio thread dropped since the last drain, its
    /// ring full: nonzero means some changes are missing. The notes held
    /// then are forgotten, since their ends may be among them: no end
    /// will be reported for them.
    pub missed: u32,
}

/// The control side of the note feed: the ring the audio thread fills,
/// and the notes it is pairing.
pub(crate) struct NoteFeed {
    ring: Arc<Ring<NoteEdge>>,
    missed: Arc<EventCounter>,
    keys: Arc<NoteKeys>,
    pairing: ControlLock<Pairing>,
}

#[derive(Default)]
struct Pairing {
    missed: EventCursor,
    /// The note each voice holds, by module key and voice.
    held: HashMap<(u32, u16), NoteChange>,
    /// The next note number, by module key.
    next: Vec<u64>,
}

impl NoteFeed {
    /// A feed, and the end the audio thread sends to. Control thread.
    pub(crate) fn new() -> (Arc<Self>, NoteOut) {
        let ring = Ring::with_capacity(NOTE_EDGE_CAPACITY);
        let missed = Arc::new(EventCounter::new());
        let out = NoteOut::new(Arc::clone(&ring), Arc::clone(&missed));
        let feed = Self {
            ring,
            missed,
            keys: Arc::default(),
            pairing: ControlLock::new(Pairing::default()),
        };
        (Arc::new(feed), out)
    }

    pub(crate) fn keys(&self) -> &Arc<NoteKeys> {
        &self.keys
    }

    /// Drains the ring and pairs what it held. Control thread.
    pub(crate) fn take(&self) -> NoteChanges {
        let mut pairing = self.pairing.lock().unwrap_or_else(|e| e.into_inner());
        let missed = pairing.missed.take(&self.missed);
        let mut notes = Vec::new();
        while let Some(edge) = self.ring.pop() {
            pairing.pair(&edge, &self.keys, &mut notes);
        }
        // The dropped edges came after those the ring held: a note still
        // held may have ended among them, and would otherwise hold forever.
        if missed > 0 {
            pairing.held.clear();
        }
        NoteChanges { notes, missed }
    }
}

impl Pairing {
    fn pair(&mut self, edge: &NoteEdge, keys: &NoteKeys, notes: &mut Vec<NoteChange>) {
        let at = NoteTime {
            frame: edge.frame,
            beats: edge.beats.is_finite().then_some(edge.beats),
        };
        // A voice plays one note at a time: whatever it held ends here.
        if let Some(mut ended) = self.held.remove(&(edge.key, edge.voice)) {
            ended.end = Some(at);
            notes.push(ended);
        }
        let pitch = 69.0 + 12.0 * (edge.frequency / 440.0).log2();
        if !edge.on || !pitch.is_finite() {
            return;
        }
        let Some(module_id) = keys.id(edge.key) else {
            return;
        };
        let key = edge.key as usize;
        if self.next.len() <= key {
            self.next.resize(key + 1, 0);
        }
        let note = NoteChange {
            module_id,
            note: self.next[key],
            voice: u32::from(edge.voice),
            pitch,
            clock: edge.clock.and_then(|clock| keys.id(clock)),
            velocity: if edge.velocity.is_nan() {
                1.0
            } else {
                edge.velocity.clamp(0.0, 1.0)
            },
            start: at,
            end: None,
        };
        self.next[key] += 1;
        notes.push(note.clone());
        self.held.insert((edge.key, edge.voice), note);
    }
}

/// Gives each module id the feed has tapped, and each clock one counts on,
/// a number the audio thread sends in its place, for as long as the
/// invention runs.
#[derive(Default)]
pub(crate) struct NoteKeys(ControlLock<Keys>);

#[derive(Default)]
struct Keys {
    ids: Vec<String>,
    by_id: HashMap<String, u32>,
}

impl NoteKeys {
    fn id(&self, key: u32) -> Option<String> {
        let keys = self.0.lock().unwrap_or_else(|e| e.into_inner());
        keys.ids.get(key as usize).cloned()
    }

    /// Taps for every module of `mirror` that plays notes, by module index.
    /// Control thread: allocates.
    pub(crate) fn taps(&self, mirror: &TopologyMirror) -> NoteTaps {
        let mut keys = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut taps = Vec::new();
        for (module_idx, (id, module)) in mirror.modules.iter().enumerate() {
            let outputs = &module.ports.outputs;
            let voices = voices(|name| outputs.iter().position(|port| port == name));
            if voices.is_empty() {
                continue;
            }
            let clock_idx = match mirror.timelines.get(module_idx) {
                Some(Context::Connected { clock, .. } | Context::Fallback { clock }) => {
                    Some(*clock)
                }
                _ => None,
            };
            let clock = clock_idx
                .and_then(|idx| Some((idx, mirror.modules.get_index(idx)?.0)))
                .map(|(idx, clock)| (idx, keys.key(clock)));
            let key = keys.key(id);
            taps.push(NoteTap::new(module_idx, key, clock, voices));
        }
        NoteTaps(taps)
    }
}

impl Keys {
    fn key(&mut self, id: &str) -> u32 {
        if let Some(&key) = self.by_id.get(id) {
            return key;
        }
        let key = self.ids.len() as u32;
        self.ids.push(id.to_string());
        self.by_id.insert(id.to_string(), key);
        key
    }
}

/// The voices a module with output ports found by `port` plays: a lone
/// `gate` with `frequency` (and `velocity`, if any) beside it, and each
/// numbered `gate.N` with its `frequency.N`.
fn voices(port: impl Fn(&str) -> Option<usize>) -> Vec<TapVoice> {
    let voice = |id: u16, suffix: &str| {
        Some(TapVoice {
            id,
            gate: port(&format!("gate{suffix}"))?,
            frequency: port(&format!("frequency{suffix}"))?,
            velocity: port(&format!("velocity{suffix}")),
            held: false,
        })
    };
    let numbered = (0..MAX_VOICES as u16).filter_map(|n| voice(n, &format!(".{n}")));
    voice(0, "").into_iter().chain(numbered).collect()
}

#[cfg(test)]
mod tests;
