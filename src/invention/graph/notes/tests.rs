//! The audio thread's note taps, on a graph driven block by block: a
//! scripted two-voice player beside a clock.

use std::sync::Arc;

use indexmap::IndexMap;

use super::super::{MasterObservers, SignalGraph};
use crate::alloc_counter::allocator_events;
use crate::control_request::{ControlIndex, RtValue};
use crate::invention::notes::{NoteChange, NoteFeed};
use crate::invention::publish::TopologyMirror;
use crate::modules::Clock;
use crate::{GraphModule, Module, MAX_BLOCK};

const RATE: u32 = 48_000;
/// At 120 BPM: a beat is 24 000 samples.
const BEAT: u64 = 24_000;
const OUTPUTS: [&str; 5] = [
    "gate.0",
    "frequency.0",
    "velocity.0",
    "gate.1",
    "frequency.1",
];

/// Plays a script: each entry holds `voice`'s gate high from `start` to
/// `end` (samples, counted from its first) at `frequency` and `velocity`.
struct Player {
    script: Vec<(usize, u64, u64, f32, f32)>,
    sample: u64,
    out: [[f32; MAX_BLOCK]; 5],
}

impl Player {
    fn new(script: Vec<(usize, u64, u64, f32, f32)>) -> Self {
        Self {
            script,
            sample: 0,
            out: [[0.0; MAX_BLOCK]; 5],
        }
    }
}

impl Module for Player {
    fn name(&self) -> &str {
        "player"
    }

    fn process(&mut self, frames: usize) -> bool {
        for i in 0..frames {
            let at = self.sample + i as u64;
            for voice in 0..2 {
                let ports = [[0, 1, 2], [3, 4, 4]][voice];
                let playing = self
                    .script
                    .iter()
                    .find(|(v, start, end, ..)| *v == voice && (*start..*end).contains(&at));
                let (gate, frequency, velocity) =
                    playing.map_or((0.0, 0.0, 0.0), |&(.., f, vel)| (1.0, f, vel));
                self.out[ports[0]][i] = gate;
                self.out[ports[1]][i] = frequency;
                if voice == 0 {
                    self.out[ports[2]][i] = velocity;
                }
            }
        }
        self.sample += frames as u64;
        true
    }

    fn inputs(&self) -> &[&str] {
        &[]
    }

    fn outputs(&self) -> &[&str] {
        &OUTPUTS
    }

    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        &mut []
    }

    fn output_block(&self, index: usize) -> &[f32] {
        &self.out[index]
    }

    fn set_input(&mut self, port: &str, _value: f32) -> Result<(), String> {
        Err(format!("no input {port}"))
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        let index = OUTPUTS
            .iter()
            .position(|p| *p == port)
            .ok_or("no such port")?;
        Ok(self.out[index][0])
    }
}

/// A graph of `player` and a clock at 120 BPM, its notes tapped as a live
/// graph's are.
fn tapped(player: Player) -> (SignalGraph, Arc<NoteFeed>) {
    let mut modules = IndexMap::new();
    modules.insert(
        "clock".to_string(),
        GraphModule::Module(Box::new(Clock::new(RATE, 120.0))),
    );
    modules.insert("player".to_string(), GraphModule::Module(Box::new(player)));
    let mut graph = SignalGraph::new(modules, Vec::new(), Vec::new(), MasterObservers::default());
    graph.recompile();
    let (feed, out) = NoteFeed::new();
    // No clock is connected: the player's beats count on the first clock.
    let mirror = TopologyMirror::of(&graph.modules, &graph.edges);
    graph.note_taps = feed.keys().taps(&mirror);
    graph.note_out = Some(out);
    (graph, feed)
}

fn render(graph: &mut SignalGraph, samples: u64) {
    let mut left = [0.0f32; 64];
    let mut right = [0.0f32; 64];
    let mut done = 0;
    while done < samples {
        let n = (samples - done).min(64) as usize;
        graph.process_block(&mut left[..n], &mut right[..n]);
        done += n as u64;
    }
}

fn beats(change: &NoteChange) -> (f64, Option<f64>) {
    let start = change.start.beats.expect("the player has a clock");
    (start, change.end.map(|end| end.beats.unwrap()))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn overlapping_voices_are_separate_notes_on_the_clock_s_beats() {
    // Voice 0 holds A4 from beat 1 to beat 3; voice 1 plays C5 from beat 2
    // to beat 2.5, inside it.
    let (mut graph, feed) = tapped(Player::new(vec![
        (0, BEAT, 3 * BEAT, 440.0, 0.75),
        (1, 2 * BEAT, 5 * BEAT / 2, 523.2511, 1.0),
    ]));
    render(&mut graph, 4 * BEAT);
    let changes = feed.take();
    assert_eq!(changes.missed, 0);
    let notes = changes.notes;
    // Starts and ends in the order they happened.
    let order: Vec<_> = notes.iter().map(|n| (n.voice, n.end.is_some())).collect();
    assert_eq!(order, [(0, false), (1, false), (1, true), (0, true)]);
    let (a, c) = (&notes[3], &notes[2]);
    assert_eq!((a.note, c.note), (0, 1), "numbered as they start");
    assert_eq!(a.module_id, "player");
    assert!((a.pitch - 69.0).abs() < 1e-4 && (c.pitch - 72.0).abs() < 1e-4);
    assert_eq!(
        (a.velocity, c.velocity),
        (0.75, 1.0),
        "no velocity port reads 1"
    );
    // Sample n (counted from 0) is the clock's n+1-th: position (n+1)/BEAT.
    let (start, end) = beats(a);
    assert!(close(start, 1.0 + 1.0 / BEAT as f64), "{start}");
    assert!(close(end.unwrap(), 3.0 + 1.0 / BEAT as f64));
    assert_eq!(a.start.frame, BEAT);
    assert_eq!(a.end.unwrap().frame, 3 * BEAT);
    let (start, end) = beats(c);
    assert!(close(start, 2.0 + 1.0 / BEAT as f64));
    assert!(close(end.unwrap(), 2.5 + 1.0 / BEAT as f64));
}

#[test]
fn a_held_note_is_reported_held_then_ended() {
    let (mut graph, feed) = tapped(Player::new(vec![(0, 100, 10 * BEAT, 220.0, 0.5)]));
    render(&mut graph, BEAT);
    let started = feed.take().notes;
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].end, None, "still held");
    render(&mut graph, BEAT);
    assert!(feed.take().notes.is_empty(), "nothing new while it holds");
    render(&mut graph, 9 * BEAT);
    let ended = feed.take().notes;
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].note, started[0].note);
    assert_eq!(ended[0].start, started[0].start);
    assert_eq!(ended[0].end.unwrap().frame, 10 * BEAT);
}

#[test]
fn beats_follow_a_tempo_change() {
    // Notes on every beat's first sample at 120 BPM, then the tempo doubles
    // at sample 2 * BEAT: the next beat falls half a beat's samples later.
    let script = (0..8).map(|n| (0, n * BEAT / 2, n * BEAT / 2 + 100, 440.0, 1.0));
    let (mut graph, feed) = tapped(Player::new(script.collect()));
    render(&mut graph, 2 * BEAT);
    let clock = graph.modules.get_index_mut(0).unwrap().1.module_mut();
    clock.apply(ControlIndex(0), RtValue::F32(240.0)).unwrap();
    render(&mut graph, 2 * BEAT);
    let notes: Vec<_> = feed
        .take()
        .notes
        .into_iter()
        .filter(|n| n.end.is_none())
        .collect();
    assert_eq!(notes.len(), 8);
    // Half a beat apart at 120, a whole beat apart at 240.
    let starts: Vec<f64> = notes.iter().map(|n| beats(n).0).collect();
    let tick = 1.0 / BEAT as f64;
    let expected = [0.0, 0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 5.0];
    for (start, expected) in starts.iter().zip(expected) {
        // The first sample is position 0 + one sample; after the change a
        // sample is two ticks.
        let tolerance = 2.0 * tick + 1e-9;
        assert!((start - expected).abs() <= tolerance, "{starts:?}");
    }
}

#[test]
fn tapping_neither_allocates_nor_frees_on_the_audio_thread() {
    let script = (0..64).map(|n| ((n % 2) as usize, n * 300, n * 300 + 200, 440.0, 1.0));
    let (mut graph, feed) = tapped(Player::new(script.collect()));
    render(&mut graph, 64);
    let mut left = [0.0f32; 64];
    let mut right = [0.0f32; 64];
    let ((), allocs, frees) = allocator_events(|| {
        for _ in 0..400 {
            graph.process_block(&mut left, &mut right);
        }
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(
        feed.take().notes.len(),
        128,
        "every start and end of 64 notes"
    );
}

#[test]
fn a_full_ring_drops_edges_and_counts_them() {
    // Far more starts and ends than the ring holds, never drained.
    let script = (0..2_000).map(|n| (0, n * 4, n * 4 + 2, 440.0, 1.0));
    let (mut graph, feed) = tapped(Player::new(script.collect()));
    render(&mut graph, 8_000);
    let changes = feed.take();
    assert_eq!(changes.missed, 4_000 - super::NOTE_EDGE_CAPACITY as u32);
    assert_eq!(feed.take().missed, 0, "counted once");
}

#[test]
fn a_reset_inside_a_segment_splits_its_count_where_it_happened() {
    // 0.01 beats a sample; the previous segment ended at 2.4, and a reset
    // at sample 20 of 64 moved the count on to beat 3.
    let span = super::ClockSpan {
        end: Some(3.0 + 43.0 * 0.01),
        rate: 0.01,
        floor: 3.0,
        previous: 2.4,
    };
    let at = |i| span.at(i, 64);
    assert!(
        close(at(0), 2.41) && close(at(19), 2.6),
        "on from before the reset"
    );
    assert!(close(at(20), 3.0) && close(at(63), 3.43), "from the reset");

    // A reset as the segment ended: all of it counts on from before.
    let ended = super::ClockSpan {
        end: None,
        rate: 0.01,
        floor: 3.0,
        previous: 2.3,
    };
    assert!(close(ended.at(63, 64), 2.94));
}

#[test]
fn the_first_sample_after_a_reset_counts_from_the_reset_despite_rounding() {
    // At 44.1 kHz and 120 BPM a sample is 1/22050 of a beat, which the
    // count back from the segment's end misses the floor by in rounding.
    let mut clock = Clock::new(44_100, 120.0);
    for _ in 0..516 {
        clock.process(64);
    }
    let timeline = |clock: &Clock| super::ClockSpan::of(clock.timeline().unwrap(), f64::NAN);
    let previous = timeline(&clock).at(63, 64);
    assert!((previous - 1.5).abs() < 0.01, "{previous}");
    clock.apply(ControlIndex(2), RtValue::Bool(true)).unwrap();
    clock.process(64);
    let span = super::ClockSpan::of(clock.timeline().unwrap(), previous);
    assert_eq!(span.at(0, 64), 2.0, "the reset's sample starts beat 2");
    assert!(close(span.at(63, 64), 2.0 + 63.0 / 22_050.0));
}
