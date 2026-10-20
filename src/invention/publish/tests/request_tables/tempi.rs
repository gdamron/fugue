//! Tempo through tables: an entry writing a clock's tempo moves what
//! waits on that clock (a request, the table's own next step) to the
//! sample the new tempo reaches it, and a tempo ramp lands as its clock
//! reaches its end, each checked against the clock on its own.

use super::super::request_beats::{level_change, submit_on, BPM as TEMPO};
use super::*;
use crate::control_request::{ramp_value, BeatSpec};
use crate::Module;

#[test]
fn a_tempo_a_table_writes_moves_a_request_waiting_on_that_clock() {
    // The table steps on `clock` and speeds `b` up at its step 1, so that
    // `b` reaches beat 1, which a request waits on, before the block ends.
    let doc = DOCUMENT.replace(
        r#"{ "id": "dac""#,
        r#"{ "id": "b", "type": "clock", "config": { "bpm": 60.0 } },
        { "id": "dac""#,
    );
    let written = beat_samples(2)[1] as u64;
    let mut twin = Twin::new(60.0);
    twin.run_to(written);
    let left = 1.0 - twin.clock.position();
    let samples = (written / 64 + 1) * 64 - written;
    let fast = (left / (samples / 2) as f64 * 60.0 * 48_000.0) as f32;
    let mut rig = Rig::new(&doc);
    rig.registry.register(DialFactory);
    rig.registry.register(TablerFactory);
    let surface = rig.live.control_surfaces.lock().unwrap()["b"].clone();
    let tempo = surface.automation("bpm").unwrap();
    let (dial, tabler) = build(&rig.registry, |_| vec![entry(1, 0, fast, &tempo)]);
    rig.live
        .edit(|change| {
            change.upsert("dial", dial);
            change.upsert("tabler", tabler);
            change.connect(edge("b", "beat", "dac", "audio"))?;
            for edge in edges("beat") {
                change.connect(edge)?;
            }
            Ok(())
        })
        .unwrap();
    rig.render(1);
    let spec = BeatSpec::Grid {
        every: 4.0,
        offset: 1.0,
        repeat: false,
    };
    submit_on(&rig, ("dial", LEVEL), RtValue::F32(0.75), "b", spec, None);
    let out = render_counted(&mut rig, blocks(1.5));
    twin.writes.push((written, TEMPO, RtValue::F32(fast)));
    let at = twin.until(|clock| clock.position() >= 1.0);
    assert!(
        at < (written / 64 + 1) * 64,
        "in the block it was written in"
    );
    assert_eq!(level_change(&out, 0.75), Some(at as usize - 64));
}

#[test]
fn a_tempo_ramp_a_table_ends_on_lands_as_its_clock_reaches_its_end() {
    for to in [300.0, 2_000.0, 20_000.0] {
        tempo_ramp_lands(to);
    }
}

/// From step 1 to step 2 a table's own clock speeds up to `to` bpm, its
/// last entry: checked against the clock on its own, written the ramp at
/// the samples the drain runs (the step's, each block's first, and the
/// one whose next sample reaches beat 2), latched as the drain latches.
fn tempo_ramp_lands(to: f32) {
    let mut rig = Rig::new(DOCUMENT);
    rig.registry.register(DialFactory);
    rig.registry.register(TablerFactory);
    let surface = rig.live.control_surfaces.lock().unwrap()["clock"].clone();
    let tempo = surface.automation("bpm").unwrap();
    let (dial, tabler) = build(&rig.registry, |_| vec![entry(1, 1, to, &tempo)]);
    rig.live
        .edit(|change| {
            change.upsert("dial", dial);
            change.upsert("tabler", tabler);
            change.connect(edge("clock", "beat", "tabler", "clock"))
        })
        .unwrap();
    let mut twin = Twin::new(BPM);
    let (mut running, mut arrived) = (false, false);
    for sample in 0..blocks(3.0) as u64 * 64 {
        let next = twin.clock.position_after(1);
        let starts = !running && next >= 1.0;
        running |= starts;
        if running && !arrived && (starts || sample.is_multiple_of(64) || next >= 2.0) {
            arrived = next >= 2.0;
            let bpm = ramp_value(BPM as f32, to, next - 1.0);
            twin.clock.timeline_mut().unwrap().latch();
            twin.clock.set_control("bpm", bpm).unwrap();
        }
        twin.tick();
        if twin.sample.is_multiple_of(64) {
            rig.render(1);
            let clock = rig.graph.modules["clock"].module().timeline().unwrap();
            let at = twin.sample;
            assert_eq!(clock.position(), twin.clock.position(), "{to} at {at}");
        }
    }
    assert!(arrived);
}

#[test]
fn an_entry_writing_its_own_clock_s_tempo_lands_the_next_on_the_new_beat() {
    let mut rig = Rig::new(DOCUMENT);
    rig.registry.register(DialFactory);
    rig.registry.register(TablerFactory);
    let surface = rig.live.control_surfaces.lock().unwrap()["clock"].clone();
    let tempo = surface.automation("bpm").unwrap();
    // A beat every 20 samples from step 0: step 1 falls in the first block.
    let (dial, tabler) = build(&rig.registry, |level| {
        vec![entry(0, 0, 144_000.0, &tempo), entry(1, 0, 0.75, level)]
    });
    rig.live
        .edit(|change| {
            change.upsert("dial", dial);
            change.upsert("tabler", tabler);
            change.connect(edge("dial", "out", "dac", "audio"))?;
            change.connect(edge("clock", "beat", "tabler", "clock"))
        })
        .unwrap();
    let out = render_counted(&mut rig, 2);
    let mut twin = Twin::new(BPM);
    twin.clock.timeline_mut().unwrap().latch();
    twin.clock.set_control("bpm", 144_000.0).unwrap();
    let at = twin.until(|clock| clock.position() >= 1.0);
    assert!(at < 64);
    assert_eq!(
        out.iter().position(|&level| level == 0.75),
        Some(at as usize)
    );
}

