//! The audio thread's side of the note feed: where the notes a module plays
//! start and end, read from its gate outputs as they were rendered.
//!
//! Every top-level module with a `gate` output and a `frequency` output
//! beside it (a sequencer), or numbered pairs of them (`gate.0` and
//! `frequency.0`, a divisi's voices), is tapped: after each segment the
//! audio thread compares each tapped gate with the level the condition
//! system reads as high ([`GATE_LEVEL`]) and hands every crossing to a
//! control thread as a [`NoteEdge`], through a preallocated ring. A full
//! ring drops the edge and counts it; nothing allocates, frees, locks or
//! waits. The control side (`crate::invention::notes`) pairs the edges
//! into notes.
//!
//! The taps are built on a control thread with each publication, from the
//! publisher's mirror, and swapped in as it installs (see
//! [`NoteTaps::take_over`]).

use std::sync::Arc;

use super::SignalGraph;
use crate::control_request::{EventCounter, Timeline, GATE_LEVEL};
use crate::spsc::{Producer, Ring};

/// Edges the ring holds between two drains. The daemon drains it every
/// 25 ms, when about a hundred notes a second would put a dozen edges in
/// it, so a full ring means the control side stopped draining.
pub(crate) const NOTE_EDGE_CAPACITY: usize = 1024;

/// One gate crossing: a note starting (`on`) or ending on one voice of a
/// tapped module.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NoteEdge {
    /// The module, by the key the feed gave its id.
    pub(crate) key: u32,
    /// The voice: 0 for a lone `gate`, `N` for `gate.N`.
    pub(crate) voice: u16,
    pub(crate) on: bool,
    /// The sample it crossed on, on the graph's transport.
    pub(crate) frame: u64,
    /// The beat it crossed on, counted on the module's clock across resets
    /// (see [`ClockSpan`]); NaN when the module has none.
    pub(crate) beats: f64,
    /// The `frequency` output as the note starts, in Hz; 0 for an end.
    pub(crate) frequency: f32,
    /// The `velocity` output as the note starts (1 with no such output); 0
    /// for an end.
    pub(crate) velocity: f32,
}

/// Where the audio thread sends note edges: the ring's producer end, and
/// the count of edges a full ring dropped.
pub(crate) struct NoteOut {
    producer: Producer<NoteEdge>,
    missed: Arc<EventCounter>,
}

impl NoteOut {
    /// Control thread: claims the producer end of `ring`.
    pub(crate) fn new(ring: Arc<Ring<NoteEdge>>, missed: Arc<EventCounter>) -> Self {
        Self {
            producer: Producer::claim(ring),
            missed,
        }
    }

    /// Sends `edge`, or counts it missed. Wait-free, allocation-free.
    #[inline]
    fn send(&mut self, edge: NoteEdge) {
        if self.producer.push(edge).is_err() {
            self.missed.record();
        }
    }
}

/// The tapped modules of one topology, by module index.
#[derive(Default)]
pub(crate) struct NoteTaps(pub(crate) Vec<NoteTap>);

/// One tapped module: its voices' ports and whether each is held.
pub(crate) struct NoteTap {
    pub(crate) module_idx: usize,
    pub(crate) key: u32,
    /// The clock its beats count on (its timeline context), by index.
    pub(crate) clock_idx: Option<usize>,
    /// That clock, by the key the feed gave its id: what tells a clock
    /// kept across an edit from another one.
    pub(crate) clock_key: Option<u32>,
    pub(crate) voices: Vec<TapVoice>,
    /// The beat its clock reached at the end of the latest segment: where
    /// a held note ends if the module goes away (see
    /// [`NoteTaps::take_over`]), and where the next segment's count
    /// continues from up to a reset inside it (see [`ClockSpan::at`]).
    last_beats: f64,
}

impl NoteTap {
    pub(crate) fn new(
        module_idx: usize,
        key: u32,
        clock: Option<(usize, u32)>,
        voices: Vec<TapVoice>,
    ) -> Self {
        Self {
            module_idx,
            key,
            clock_idx: clock.map(|(idx, _)| idx),
            clock_key: clock.map(|(_, key)| key),
            voices,
            last_beats: f64::NAN,
        }
    }
}

/// One voice's output ports, by index, and whether its gate is high.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TapVoice {
    pub(crate) id: u16,
    pub(crate) gate: usize,
    pub(crate) frequency: usize,
    pub(crate) velocity: Option<usize>,
    pub(crate) held: bool,
}

impl NoteTaps {
    /// Audio thread, as an edit installs these taps over `previous`: each
    /// voice still tapped, on the same clock, keeps holding what it held,
    /// so a note sounding across the edit is one note. Each note held on a
    /// voice no longer tapped (its module removed, or rebuilt without that
    /// voice) or whose module now counts on another clock ends at `frame`;
    /// a gate still high starts a new note, on the new clock, as the next
    /// segment is tapped. Allocation-free.
    pub(crate) fn take_over(&mut self, previous: &NoteTaps, out: &mut NoteOut, frame: u64) {
        for old in &previous.0 {
            let mut new = (self.0.iter_mut())
                .find(|tap| tap.key == old.key && tap.clock_key == old.clock_key);
            if let Some(tap) = new.as_deref_mut() {
                tap.last_beats = old.last_beats;
            }
            for voice in old.voices.iter().filter(|voice| voice.held) {
                let kept = (new.as_deref_mut())
                    .and_then(|tap| tap.voices.iter_mut().find(|v| v.id == voice.id));
                match kept {
                    Some(kept) => kept.held = true,
                    None => out.send(NoteEdge {
                        key: old.key,
                        voice: voice.id,
                        on: false,
                        frame,
                        beats: old.last_beats,
                        frequency: 0.0,
                        velocity: 0.0,
                    }),
                }
            }
        }
    }
}

/// A clock's beats across the segment just rendered, counted across its
/// resets: the beats begun before its latest reset plus its position, so
/// a looping clock counts on rather than folding the history back.
#[derive(Clone, Copy)]
struct ClockSpan {
    /// The count at the segment's last sample; `None` when the clock reset
    /// as the segment ended, so that none of it counts from the reset.
    end: Option<f64>,
    /// Beats per sample at the tempo it played the segment at.
    rate: f64,
    /// Where the count stood at the latest reset: nothing after the reset
    /// is below it, nothing before it reaches it.
    floor: f64,
    /// The count at the previous segment's last sample (NaN for none).
    previous: f64,
}

impl ClockSpan {
    fn of(timeline: &dyn Timeline, previous: f64) -> Self {
        let position = timeline.position();
        let (end, rate) = if position < 0.0 {
            // Its next sample starts beat 0; the tempo is the next steps'.
            let next = timeline.position_after(1);
            (None, timeline.position_after(2) - next)
        } else {
            (Some(position), timeline.position_after(1) - position)
        };
        let floor = timeline.beats_before() as f64;
        Self {
            end: end.map(|position| floor + position),
            rate,
            floor,
            previous,
        }
    }

    /// The count at sample `i` of a segment `frames` long: back from the
    /// end, or, for a sample before a reset inside the segment (a
    /// condition or a table can reset a clock mid-segment), on from the
    /// previous segment. Exact while the tempo holds through the segment.
    fn at(&self, i: usize, frames: usize) -> f64 {
        if let Some(end) = self.end {
            let back = end - (frames - 1 - i) as f64 * self.rate;
            // A sample before the reset falls a whole sample short of the
            // floor; the reset's own sample lands on it, give or take
            // rounding.
            if back >= self.floor - 0.5 * self.rate {
                return back.max(self.floor);
            }
        }
        if self.previous.is_nan() {
            return self.floor;
        }
        (self.previous + (i + 1) as f64 * self.rate).min(self.floor)
    }
}

impl SignalGraph {
    /// Audio thread, after each segment of `frames` samples: sends an edge
    /// for every tapped gate that crossed [`GATE_LEVEL`] in it. Reads only
    /// the output buffers and the clocks' timelines; allocation-, free- and
    /// lock-free.
    pub(super) fn tap_notes(&mut self, frames: usize) {
        let Some(out) = self.note_out.as_mut() else {
            return;
        };
        if frames == 0 {
            return;
        }
        let start = self.current_sample - frames as u64;
        let capacity = self.block_capacity;
        for tap in self.note_taps.0.iter_mut() {
            let Some(outputs) = self.out_bufs.get(tap.module_idx) else {
                continue;
            };
            let clock = (tap.clock_idx)
                .and_then(|clock| self.modules.get_index(clock))
                .and_then(|(_, instance)| instance.module().timeline())
                .map(|timeline| ClockSpan::of(timeline, tap.last_beats));
            let beats = |i: usize| clock.map_or(f64::NAN, |clock| clock.at(i, frames));
            for voice in tap.voices.iter_mut() {
                let port = |port: usize| outputs.get(port * capacity..port * capacity + frames);
                let (Some(gate), Some(frequency)) = (port(voice.gate), port(voice.frequency))
                else {
                    continue;
                };
                let velocity = voice.velocity.and_then(port);
                for i in 0..frames {
                    let high = gate[i] >= GATE_LEVEL;
                    if high == voice.held {
                        continue;
                    }
                    voice.held = high;
                    out.send(NoteEdge {
                        key: tap.key,
                        voice: voice.id,
                        on: high,
                        frame: start + i as u64,
                        beats: beats(i),
                        frequency: if high { frequency[i] } else { 0.0 },
                        velocity: match (high, velocity) {
                            (false, _) => 0.0,
                            (true, Some(velocity)) => velocity[i],
                            (true, None) => 1.0,
                        },
                    });
                }
            }
            tap.last_beats = beats(frames - 1);
        }
    }
}

#[cfg(test)]
mod tests;
