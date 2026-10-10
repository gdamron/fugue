//! Requests timed in beats on a live graph: each applies on the exact
//! sample its clock reaches the beat (the sample the clock's `beat` gate
//! rises on, for a whole beat), at any tempo, through tempo changes and
//! resets made while it waits, and every block stays allocation- and
//! free-free. A twin clock, run sample by sample beside the graph, is the
//! oracle.

use super::requests::{counted_block, outcomes, submit};
use super::*;
use crate::control_request::{
    BeatSpec, BeatTime, ControlIndex, Outcome, Refusal, Request, RequestId, RequestValue, RtValue,
    Timeline, When,
};
use crate::invention::publish::publisher::BEAT_WATCH_CAPACITY;
use crate::modules::Clock;
use crate::test_support::dial::{DialFactory, DIAL, LEVEL};
use crate::Module;

pub(super) const BPM: ControlIndex = ControlIndex(0);
pub(super) const RESET: ControlIndex = ControlIndex(2);

/// A clock at `bpm` from sample 0 and a dial (level 0.25), both into an
/// unclipped dac: the output is the clock's gate plus the dial's level.
pub(super) fn beat_rig(bpm: f64) -> Rig {
    let mut rig = Rig::new(&format!(
        r#"{{
        "version": "1.0.0",
        "modules": [
            {{ "id": "clock", "type": "clock", "config": {{ "bpm": {bpm} }} }},
            {{ "id": "dac", "type": "dac", "config": {{ "soft_clip": false }} }}
        ],
        "connections": [{{ "from": "clock", "from_port": "beat", "to": "dac", "to_port": "audio" }}]
    }}"#
    ));
    rig.registry.register(DialFactory);
    let dial = rig.build("dial", DIAL, serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("dial", dial);
            change.connect(edge("dial", "out", "dac", "audio"))
        })
        .unwrap();
    rig
}

/// Submits a request as a front door would: `value` for `control` of
/// `module_id`, at `spec` on the clock (or on `on`, by id).
pub(super) fn submit_on(
    rig: &Rig,
    (module_id, control): (&str, ControlIndex),
    value: RtValue,
    on: &str,
    spec: BeatSpec,
    ttl: Option<u64>,
) -> RequestId {
    let mut publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target(module_id, control).unwrap();
    let clock = publisher.control_target(on, ControlIndex(0)).unwrap();
    let mut request = Request::new(target, RequestValue::Value(value));
    request.when = When::Beat(BeatTime {
        clock: clock.module_idx as u32,
        spec,
    });
    request.ttl = ttl;
    request.event = control == RESET && module_id == "clock";
    let id = rig.live.requests.submit(request).unwrap();
    publisher.note_written();
    id
}

/// Sets the dial's level to `level` at `spec` on the clock.
pub(super) fn submit_level(rig: &Rig, level: f32, spec: BeatSpec) -> RequestId {
    submit_on(
        rig,
        ("dial", LEVEL),
        RtValue::F32(level),
        "clock",
        spec,
        None,
    )
}

/// The outcomes settled so far for request `id`.
pub(super) fn outcomes_of(rig: &mut Rig, id: RequestId) -> Vec<(RequestId, Outcome)> {
    let mut all = outcomes(rig);
    all.retain(|(got, _)| *got == id);
    all
}

/// Writes `value` to the clock's `control` at sample `at`.
pub(super) fn submit_clock_at(rig: &Rig, control: ControlIndex, value: RtValue, at: u64) {
    let mut publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("clock", control).unwrap();
    let mut request = Request::new(target, RequestValue::Value(value));
    request.when = When::AtSample(at);
    request.event = control == RESET;
    rig.live.requests.submit(request).unwrap();
    publisher.note_written();
}

/// Renders `blocks` blocks, each allocation- and free-free, returning the
/// left channel.
pub(super) fn render_counted(rig: &mut Rig, blocks: usize) -> Vec<f32> {
    let mut out = Vec::new();
    for _ in 0..blocks {
        let mut left = [0.0f32; 64];
        let mut right = [0.0f32; 64];
        let ((), allocs, frees) = crate::alloc_counter::allocator_events(|| {
            rig.graph.process_block(&mut left, &mut right)
        });
        assert_eq!((allocs, frees), (0, 0));
        out.extend_from_slice(&left);
    }
    out
}

/// The rig's clock, played sample by sample, with writes at given samples.
pub(super) struct Twin {
    pub(super) clock: Clock,
    /// Samples output so far: the index of the next.
    pub(super) sample: u64,
    pub(super) writes: Vec<(u64, ControlIndex, RtValue)>,
}

impl Twin {
    pub(super) fn new(bpm: f64) -> Self {
        Self {
            clock: Clock::new(48_000, bpm),
            sample: 0,
            writes: Vec::new(),
        }
    }

    /// Outputs one sample, after any write due at it.
    pub(super) fn tick(&mut self) {
        for (at, control, value) in &self.writes {
            if *at == self.sample {
                let key = if *control == BPM { "bpm" } else { "reset" };
                let value = match value {
                    RtValue::F32(value) => *value,
                    _ => 1.0,
                };
                self.clock.set_control(key, value).unwrap();
            }
        }
        self.clock.tick();
        self.sample += 1;
    }

    pub(super) fn run_to(&mut self, sample: u64) {
        while self.sample < sample {
            self.tick();
        }
    }

    /// The sample at which `reached` first holds, from the next.
    pub(super) fn until(&mut self, reached: impl Fn(&Clock) -> bool) -> u64 {
        loop {
            self.tick();
            if reached(&self.clock) {
                return self.sample - 1;
            }
        }
    }
}

/// Where in `out` (gate plus a level below 1) the level first becomes
/// `level`.
pub(super) fn level_change(out: &[f32], level: f32) -> Option<usize> {
    out.iter()
        .position(|sample| sample - sample.floor() == level)
}

/// Blocks of 64 enough for `beats` beats at `bpm` and one more.
pub(super) fn blocks_for(bpm: f64, beats: f64) -> usize {
    ((beats + 1.0) * 48_000.0 * 60.0 / bpm / 64.0) as usize + 2
}

/// Tempi with fractional samples per beat at 48 kHz, and a whole one.
const TEMPI: [f64; 4] = [120.0, 97.0, 133.3, 71.25];

#[test]
fn a_request_n_beats_ahead_applies_on_the_exact_sample() {
    for bpm in TEMPI {
        for (blocks, beats) in [(1, 4.0), (7, 4.0), (40, 1.5), (3, 0.0)] {
            let mut rig = beat_rig(bpm);
            rig.render(blocks);
            let start = rig.graph.current_sample;
            let id = submit_level(&rig, 0.5, BeatSpec::After(beats));
            let out = render_counted(&mut rig, blocks_for(bpm, f64::from(beats)));

            let mut twin = Twin::new(bpm);
            twin.run_to(start);
            let goal = twin.clock.position().max(0.0) + f64::from(beats);
            let at = twin.until(|clock| clock.position() >= goal);
            let applied = Outcome::Applied { at };
            assert_eq!(outcomes(&mut rig), [(id, applied)], "{bpm} bpm, +{beats}");
            let heard = level_change(&out, 0.5).map(|i| start + i as u64);
            assert_eq!(heard, Some(at), "{bpm} bpm, +{beats}");
        }
    }
}

#[test]
fn a_whole_beat_lands_with_the_gate_rising() {
    let mut rig = beat_rig(97.0);
    // From a clock about to start: beat 4 is the 4th rise after beat 0's.
    submit_level(&rig, 0.5, BeatSpec::After(4.0));
    let out = render_counted(&mut rig, blocks_for(97.0, 5.0));
    let rises: Vec<usize> = (1..out.len())
        .filter(|&i| out[i] >= 1.0 && out[i - 1] < 1.0)
        .collect();
    assert_eq!(level_change(&out, 0.5), Some(rises[3]), "{rises:?}");
}

#[test]
fn a_tempo_change_while_a_request_waits_is_honoured() {
    for bpm in TEMPI {
        for (offset, new_bpm) in [(1_000, bpm * 1.5), (7_777, bpm * 0.6), (20_000, 333.3)] {
            let mut rig = beat_rig(bpm);
            rig.render(2);
            let start = rig.graph.current_sample;
            let id = submit_level(&rig, 0.5, BeatSpec::After(6.0));
            let change_at = start + offset;
            submit_clock_at(&rig, BPM, RtValue::F32(new_bpm as f32), change_at);
            let out = render_counted(&mut rig, blocks_for(bpm.min(new_bpm), 6.0));

            let mut twin = Twin::new(bpm);
            twin.writes
                .push((change_at, BPM, RtValue::F32(new_bpm as f32)));
            twin.run_to(start);
            let goal = twin.clock.position().max(0.0) + 6.0;
            let at = twin.until(|clock| clock.position() >= goal);
            assert_eq!(outcomes_of(&mut rig, id), [(id, Outcome::Applied { at })]);
            let heard = level_change(&out, 0.5).map(|i| start + i as u64);
            assert_eq!(heard, Some(at), "{bpm} -> {new_bpm} bpm at {change_at}");
        }
    }
}

#[test]
fn a_reset_while_counting_beats_counts_as_reaching_the_next_beat() {
    let mut rig = beat_rig(120.0);
    rig.render(10);
    let start = rig.graph.current_sample;
    let id = submit_level(&rig, 0.5, BeatSpec::After(4.0));
    // Half a beat in, the reset begins beat 1 of the count again from the
    // clock's beat 0: three more beats to go.
    let reset_at = start + 12_000;
    submit_clock_at(&rig, RESET, RtValue::Bool(true), reset_at);
    render_counted(&mut rig, blocks_for(120.0, 4.0));

    let mut twin = Twin::new(120.0);
    twin.writes.push((reset_at, RESET, RtValue::Bool(true)));
    twin.run_to(start);
    let goal = twin.clock.position() + 4.0;
    let at = twin.until(|clock| clock.beats_before() as f64 + clock.position() >= goal);
    assert_eq!(outcomes_of(&mut rig, id), [(id, Outcome::Applied { at })]);
}

#[test]
fn a_span_counts_from_before_a_reset_at_the_sample_it_arrives() {
    for reset_first in [false, true] {
        let mut rig = beat_rig(120.0);
        rig.render(1);
        // Half a beat in, once the next block starts.
        rig.render(186);
        let now = rig.graph.current_sample;
        let reset = || submit_clock_at(&rig, RESET, RtValue::Bool(true), now);
        if reset_first {
            reset();
        }
        let id = submit_level(&rig, 0.5, BeatSpec::After(0.25));
        if !reset_first {
            reset();
        }
        render_counted(&mut rig, 1);
        // The reset begins the next whole beat, past the span's end: it
        // applies at the reset, not a quarter beat after it.
        let applied = Outcome::Applied { at: now };
        assert_eq!(outcomes_of(&mut rig, id), [(id, applied)], "{reset_first}");
    }
}

#[test]
fn removing_the_clock_or_the_target_refuses_what_waits_on_it() {
    let mut rig = beat_rig(120.0);
    rig.render(1);
    let on_clock = submit_level(&rig, 0.5, BeatSpec::After(4.0));
    let to_clock = submit_on(
        &rig,
        ("clock", BPM),
        RtValue::F32(90.0),
        "clock",
        BeatSpec::After(8.0),
        None,
    );
    rig.render(1);
    rig.live
        .edit(|change| {
            change.remove("clock");
            Ok(())
        })
        .unwrap();
    render_counted(&mut rig, 1);
    assert_eq!(
        outcomes(&mut rig),
        [
            (on_clock, Outcome::Refused(Refusal::TimelineGone)),
            (to_clock, Outcome::Refused(Refusal::TargetGone)),
        ]
    );
}

#[test]
fn a_tempo_change_on_a_beat_leaves_that_beat_where_it_was() {
    for new_bpm in [0.0, 60.0, 240.0] {
        for tempo_first in [true, false] {
            let mut rig = beat_rig(120.0);
            let tempo = || {
                let control = ("clock", BPM);
                let value = RtValue::F32(new_bpm);
                submit_on(&rig, control, value, "clock", BeatSpec::After(4.0), None)
            };
            let level = || submit_level(&rig, 0.5, BeatSpec::After(4.0));
            let ids = if tempo_first {
                [tempo(), level()]
            } else {
                [level(), tempo()]
            };
            let out = render_counted(&mut rig, blocks_for(120.0, 4.0));
            let context = format!("{new_bpm} bpm, tempo first: {tempo_first}");
            // Beat 4 at 120 bpm is sample 95999: its gate rises there, with
            // the level, and the new tempo runs from the sample after.
            let applied = Outcome::Applied { at: 95_999 };
            let got: Vec<_> = outcomes(&mut rig);
            assert_eq!(got, ids.map(|id| (id, applied)), "{context}");
            assert_eq!(out[95_999], 1.5, "{context}");
            assert!(out[95_998] < 1.0, "{context}");
            let mut twin = Twin::new(120.0);
            twin.run_to(96_000);
            let clock = rig.graph.modules["clock"].module().timeline().unwrap();
            let after =
                4.0 + (rig.graph.current_sample - 96_000) as f64 * f64::from(new_bpm) / 2_880_000.0;
            assert_eq!(twin.clock.position(), 4.0, "{context}");
            assert!((clock.position() - after).abs() < 1e-9, "{context}");
        }
    }
}

#[test]
fn full_watches_refuse_a_beat_request_without_holding_the_queue() {
    let mut rig = beat_rig(120.0);
    rig.render(1);
    let far = BeatSpec::After(1.0e6);
    for _ in 0..BEAT_WATCH_CAPACITY {
        submit_level(&rig, 0.5, far);
    }
    let refused = submit_level(&rig, 0.5, far);
    let now = submit(&rig, "dial", LEVEL.0, 0.75, When::Now);
    let out = render_counted(&mut rig, 1);
    assert_eq!(
        outcomes(&mut rig),
        [
            (refused, Outcome::Refused(Refusal::PendingFull)),
            (now, Outcome::Applied { at: 64 }),
        ]
    );
    assert_eq!(level_change(&out, 0.75), Some(0));
}

#[test]
fn a_beat_on_a_module_with_no_timeline_or_past_its_ttl_is_refused() {
    let mut rig = beat_rig(120.0);
    rig.render(1);
    let level = ("dial", LEVEL);
    let value = RtValue::F32(0.5);
    let on_dial = submit_on(&rig, level, value, "dial", BeatSpec::After(1.0), None);
    let ttl = submit_on(
        &rig,
        level,
        value,
        "clock",
        BeatSpec::After(1.0),
        Some(10_000),
    );
    let bad = [-1.0, f32::INFINITY]
        .map(|beats| submit_on(&rig, level, value, "clock", BeatSpec::After(beats), None));
    render_counted(&mut rig, blocks_for(120.0, 1.0));
    assert_eq!(
        outcomes(&mut rig),
        [
            (bad[0], Outcome::Refused(Refusal::Invalid)),
            (bad[1], Outcome::Refused(Refusal::Invalid)),
            (on_dial, Outcome::Refused(Refusal::NoTimeline)),
            (ttl, Outcome::Refused(Refusal::Expired)),
        ]
    );
}

#[test]
fn requests_wait_on_a_clock_at_its_own_speed_without_allocating() {
    let mut rig = beat_rig(22_500.0);
    rig.render(2);
    // At beat 1 exactly: beats 2 to 10 are the samples ending each 128.
    let ids: Vec<RequestId> = (1..=9)
        .map(|beats| submit_level(&rig, 0.5, BeatSpec::After(beats as f32)))
        .collect();
    for _ in 0..20 {
        assert_eq!(counted_block(&mut rig), (0, 0));
    }
    let clock = rig.graph.modules["clock"].module().timeline().unwrap();
    assert!(clock.position() > 10.0);
    let expected: Vec<(RequestId, Outcome)> = (ids.iter().zip(1..))
        .map(|(id, beats)| {
            (
                *id,
                Outcome::Applied {
                    at: 128 * (beats + 1) - 1,
                },
            )
        })
        .collect();
    assert_eq!(outcomes(&mut rig), expected);
}
