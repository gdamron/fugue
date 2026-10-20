//! Ramps in beats on a scripted timeline: what a ramp writes for each
//! position of its clock, what each running writes at each segment start,
//! that it arrives exactly on its end sample, what ends it early, and that
//! none of it allocates or frees.

use super::*;
use crate::control_request::hold::Rank;
use crate::control_request::ramps::{ramp_value, BeatRamp, Ramps};
use crate::control_request::request::Source;
use crate::test_support::table::TestTable;

/// A clock at `position`, a beat every 128 samples.
fn at(position: f64, beats_before: u64) -> Scripted {
    Scripted {
        position,
        rate: 1.0 / 128.0,
        beats_before,
    }
}

/// A sample before `beat`: the clock's next sample is on it.
fn before(beat: f64) -> f64 {
    beat - 1.0 / 128.0
}

#[test]
fn a_ramp_glides_from_its_start_and_arrives_exactly_at_its_end() {
    let ramp = BeatRamp::new(0.25, 1.0, 1.0, 2.0, 0);
    assert_eq!(ramp.next(&at(before(1.0), 0)), Some((0.25, false)));
    let halfway = ramp_value(0.25, 1.0, 0.5);
    assert_eq!(ramp.next(&at(before(2.0), 0)), Some((halfway, false)));
    let short = ramp.next(&at(before(3.0) - 1.0 / 128.0, 0));
    assert!(matches!(short, Some((value, false)) if value < 1.0));
    assert_eq!(ramp.next(&at(before(3.0), 0)), Some((1.0, true)));
    assert_eq!(ramp.next(&at(5.0, 0)), Some((1.0, true)));
    // Its end sample is 256 after beat 1's, the one the clock plays next.
    assert_eq!(ramp.samples_to_end(&at(before(1.0), 0)), Some(257));
}

#[test]
fn a_ramp_carries_on_from_the_next_whole_beat_after_a_reset() {
    let ramp = BeatRamp::new(0.0, 1.0, 1.0, 2.0, 0);
    // Reset at position 1.5: beat 2 is begun, so the count is 2 from the
    // clock's new beat 0, and the ramp has a beat to go.
    let reset = at(before(0.0), 2);
    assert_eq!(ramp.next(&reset), Some((0.5, false)));
    assert_eq!(ramp.samples_to_end(&reset), Some(129));
    assert_eq!(ramp.next(&at(before(1.0), 2)), Some((1.0, true)));
    // Counted from a clock that had begun beats already.
    let late = BeatRamp::new(0.0, 1.0, 1.0, 2.0, 2);
    assert_eq!(late.next(&reset), Some((0.0, false)));
}

#[test]
fn a_ramp_on_a_clock_whose_position_is_no_number_writes_nothing() {
    let ramp = BeatRamp::new(0.0, 1.0, 1.0, 2.0, 0);
    assert_eq!(ramp.next(&at(f64::NAN, 0)), None);
}

#[test]
fn ramp_values_stay_between_the_endpoints_and_finite() {
    assert_eq!(ramp_value(0.25, 1.0, -1.0), 0.25);
    assert_eq!(ramp_value(0.25, 1.0, 2.0), 1.0);
    assert_eq!(ramp_value(1.0, -1.0, 0.5), 0.0);
    assert_eq!(ramp_value(f32::MIN, f32::MAX, 0.5), 0.0);
    assert_eq!(ramp_value(f32::MIN, f32::MAX, 1.0), f32::MAX);
    assert_eq!(ramp_value(f32::MIN, f32::MAX, f64::NAN), f32::MIN);
    for step in 0..=64 {
        let value = ramp_value(f32::MIN, f32::MAX, f64::from(step) / 64.0);
        assert!(value.is_finite(), "{step}");
    }
}

/// A scripted clock at module 0 and a number control on module 1,
/// recording every write with its sample and standing.
pub(super) struct RampHost {
    pub(super) clock: Scripted,
    pub(super) now: u64,
    pub(super) value: f32,
    pub(super) writes: Vec<(u64, f32)>,
    pub(super) standings: Vec<Option<Standing>>,
    /// Refuses every write as a higher tier's hold would.
    pub(super) held: bool,
    /// Another write has taken the control since the ramp's.
    pub(super) taken: bool,
    /// Writes set the clock's rate, in beats per 128 samples, recording
    /// the position and rate each found.
    pub(super) tempo: Option<Vec<(f64, f64)>>,
    /// The table module 2 plays.
    pub(super) table: Option<TestTable>,
}

impl RampHost {
    pub(super) fn new(value: f32) -> Self {
        Self {
            clock: Scripted {
                position: 0.0,
                rate: 1.0 / 128.0,
                beats_before: 0,
            },
            now: 0,
            value,
            writes: Vec::with_capacity(1024),
            standings: Vec::with_capacity(1024),
            held: false,
            taken: false,
            tempo: None,
            table: None,
        }
    }

    /// Plays `samples` samples.
    pub(super) fn play(&mut self, samples: u64) {
        self.clock.position = self.clock.position_after(samples);
        self.now += samples;
    }
}

impl BeatHost for RampHost {
    fn timeline(&self, module_idx: usize) -> Option<&dyn Timeline> {
        (module_idx == 0).then_some(&self.clock as &dyn Timeline)
    }

    fn latch(&mut self, _module_idx: usize) {}

    fn current(&mut self, target: &ControlTarget) -> Option<f32> {
        (target.module_idx == 1).then_some(self.value)
    }

    fn kept(&self, _target: &ControlTarget, _standing: Standing) -> bool {
        !self.held && !self.taken
    }

    fn overlaps(&self, a: &ControlTarget, b: &ControlTarget) -> bool {
        (a.module_idx, a.control) == (b.module_idx, b.control)
    }

    fn table(&self, module_idx: usize) -> Option<&dyn BeatTable> {
        let table = self.table.as_ref().filter(|_| module_idx == 2)?;
        Some(table as &dyn BeatTable)
    }

    fn apply(
        &mut self,
        _target: &ControlTarget,
        standing: Option<Standing>,
        value: RequestValue,
        _retirer: &mut Retirer,
    ) -> Result<(), Outcome> {
        if self.held {
            return Err(Outcome::Overridden);
        }
        let RequestValue::Value(RtValue::F32(value)) = value else {
            return Err(Outcome::Refused(Refusal::Unsupported));
        };
        if let Some(found) = &mut self.tempo {
            found.push((self.clock.position_after(1), self.clock.rate));
            self.clock.rate = f64::from(value) / 128.0;
        }
        self.value = value;
        self.writes.push((self.now, value));
        self.standings.push(standing);
        Ok(())
    }
}

/// Control 0 of module 1.
pub(super) fn control() -> ControlTarget {
    ControlTarget {
        generation: INSTALLED,
        module_idx: 1,
        control: ControlIndex(0),
    }
}

/// Plays to sample 127, where the clock's next sample is at beat 1, and
/// starts a ramp there from the control's value to `to` over `beats`, as
/// automation's write. Sample n is at position (n + 1) / 128.
fn begin(ramps: &mut Ramps, host: &mut RampHost, to: f32, beats: f64) -> Standing {
    host.play(127 - host.now);
    let mut request = request(1, 1, 0.0);
    request.source = Source::Automation;
    let standing = Standing::of(&request, host.now).unwrap();
    let ramp = BeatRamp::new(host.value, to, 1.0, beats, 0);
    ramps.start((control(), 0), standing, ramp, &*host);
    standing
}

/// Steps the ramps at each segment start until sample `until`, a segment
/// ending just before the nearest arrives, or after 64 samples.
fn run(ramps: &mut Ramps, host: &mut RampHost, store: &mut PendingStore, until: u64) {
    while host.now < until {
        let arrives = ramps.step(INSTALLED, host, &mut store.outcomes.retirer);
        host.play(arrives.unwrap_or(64).min(64).min(until - host.now));
    }
}

#[test]
fn a_ramp_arrives_exactly_on_its_end_sample() {
    let (mut store, _outcomes) = store();
    let mut ramps = Ramps::new(4);
    let mut host = RampHost::new(0.25);
    let standing = begin(&mut ramps, &mut host, 1.0, 2.0);
    run(&mut ramps, &mut host, &mut store, 1024);
    assert!(ramps.is_empty(), "it ends silently once it arrives");
    let (first, last) = (host.writes[0], *host.writes.last().unwrap());
    assert_eq!(first, (127, 0.25));
    assert_eq!(last, (383, 1.0), "it lands exactly on beat 3");
    for &(sample, value) in &host.writes[1..host.writes.len() - 1] {
        let position = (sample + 1) as f64 / 128.0;
        assert!((127..383).contains(&sample), "{sample}");
        assert_eq!(value, ramp_value(0.25, 1.0, (position - 1.0) / 2.0));
    }
    // Every write stands as the ramp's start did.
    assert!(host.standings.iter().all(|&got| got == Some(standing)));
    assert_eq!(standing.rank, Rank::new(Source::Automation, 0));
}

#[test]
fn a_ramp_a_higher_tier_holds_is_dropped_without_a_word() {
    let (mut store, _outcomes) = store();
    let mut ramps = Ramps::new(4);
    let mut host = RampHost::new(0.0);
    begin(&mut ramps, &mut host, 1.0, 2.0);
    run(&mut ramps, &mut host, &mut store, 200);
    host.held = true;
    run(&mut ramps, &mut host, &mut store, 1024);
    assert!(ramps.is_empty());
    assert!(host.writes.iter().all(|&(sample, _)| sample < 200));
}

#[test]
fn a_ramp_whose_control_another_write_took_ends_without_a_write() {
    let (mut store, _outcomes) = store();
    let mut ramps = Ramps::new(4);
    let mut host = RampHost::new(0.0);
    begin(&mut ramps, &mut host, 1.0, 2.0);
    run(&mut ramps, &mut host, &mut store, 200);
    // A scheduler's entry, say: automation's next event on the control.
    host.taken = true;
    run(&mut ramps, &mut host, &mut store, 1024);
    assert!(ramps.is_empty());
    assert!(host.writes.iter().all(|&(sample, _)| sample < 200));
}

#[test]
fn a_new_ramp_on_its_control_replaces_a_running_one() {
    let (mut store, _outcomes) = store();
    let mut ramps = Ramps::new(4);
    let mut host = RampHost::new(0.0);
    begin(&mut ramps, &mut host, 1.0, 4.0);
    run(&mut ramps, &mut host, &mut store, 255);
    // At beat 2, a quarter of the way, another from there to -1 by beat 3.
    let standing = Standing::of(&request(2, 1, 0.0), 255).unwrap();
    let ramp = BeatRamp::new(host.value, -1.0, 2.0, 1.0, 0);
    ramps.start((control(), 0), standing, ramp, &host);
    run(&mut ramps, &mut host, &mut store, 1024);
    assert!(ramps.is_empty());
    assert_eq!(*host.writes.last().unwrap(), (383, -1.0));
    let after = host.writes.iter().filter(|&&(sample, _)| sample >= 255);
    assert!(after.clone().all(|&(_, value)| value <= 0.25));
}

#[test]
fn a_ramp_carries_on_through_a_reset_of_its_clock() {
    let (mut store, _outcomes) = store();
    let mut ramps = Ramps::new(4);
    let mut host = RampHost::new(0.0);
    begin(&mut ramps, &mut host, 1.0, 2.0);
    run(&mut ramps, &mut host, &mut store, 192);
    // A reset at position 1.5: beat 2 is begun, so the count jumps there
    // and the ramp, half done, has a beat to go from the new beat 0.
    host.clock.beats_before += 2;
    host.clock.position = -1.0 / 128.0;
    run(&mut ramps, &mut host, &mut store, 1024);
    assert!(ramps.is_empty());
    assert_eq!(*host.writes.last().unwrap(), (192 + 128, 1.0));
}

#[test]
fn a_ramp_of_its_own_clocks_tempo_still_lands_on_its_end_sample() {
    for (from, to) in [(1.0, 8.0), (8.0, 1.0)] {
        let (mut store, _outcomes) = store();
        let mut ramps = Ramps::new(4);
        let mut host = RampHost::new(from);
        host.clock.rate = f64::from(from) / 128.0;
        host.tempo = Some(Vec::with_capacity(1024));
        // At beat 1 on a clock running `from` beats per 128 samples.
        host.play(128 / from as u64 - 1);
        let standing = Standing::of(&request(1, 1, 0.0), host.now).unwrap();
        let ramp = BeatRamp::new(from, to, 1.0, 1.0, 0);
        ramps.start((control(), 0), standing, ramp, &host);
        run(&mut ramps, &mut host, &mut store, 4096);
        let found = host.tempo.take().unwrap();
        let &(reached, rate) = found.last().unwrap();
        assert_eq!(host.writes.last().unwrap().1, to);
        // Its last write is on the first sample the clock reaches beat 2.
        assert!(
            reached >= 2.0 && reached - rate < 2.0,
            "{from} -> {to}: {reached}"
        );
    }
}

#[test]
fn a_ramp_whose_target_or_clock_goes_ends_silently() {
    let mut ramps = Ramps::new(4);
    let host = RampHost::new(0.0);
    let standing = Standing::of(&request(1, 1, 0.0), 0).unwrap();
    let ramp = BeatRamp::new(0.0, 1.0, 0.0, 1.0, 0);
    let old = ControlTarget {
        generation: INSTALLED - 1,
        ..control()
    };
    ramps.start((old, 0), standing, ramp, &host);
    ramps.remap(INSTALLED, |_, module_idx| Some(module_idx));
    assert_eq!(ramps.clocks(INSTALLED).collect::<Vec<_>>(), [0]);
    ramps.remap(INSTALLED + 1, |_, module_idx| {
        (module_idx == 1).then_some(1)
    });
    assert!(ramps.is_empty(), "its clock went");
}

#[test]
fn running_ramps_neither_allocate_nor_free() {
    let (mut store, _outcomes) = store();
    let mut ramps = Ramps::new(4);
    let mut host = RampHost::new(0.0);
    let ((), allocs, frees) = allocator_events(|| {
        begin(&mut ramps, &mut host, 1.0, 2.0);
        run(&mut ramps, &mut host, &mut store, 1024);
    });
    assert_eq!((allocs, frees), (0, 0));
    assert!(ramps.is_empty());
}
