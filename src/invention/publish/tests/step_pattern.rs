//! The step sequencer's pattern on a live graph: a write applies from the
//! next block and reads back as written, and clock edges render without
//! allocating, freeing or waiting while a control thread writes patterns.
//! The pattern each write replaces is retired: process_block runs in an
//! audio-thread scope, where dropping it panics in debug builds.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

use super::migrated::{block_with_locks_held, read, write, SEQUENCER};
use super::requests::counted_block;
use super::*;
use crate::music::Note;
use crate::ControlValue;

fn hz(note: u8) -> f32 {
    Note::new(note).frequency()
}

fn frequency(rig: &Rig) -> f32 {
    rig.graph.modules["seq"]
        .module()
        .get_output("frequency")
        .unwrap()
}

/// One block with the sequencer's clock at `level`, counted.
fn clocked_block(rig: &mut Rig, level: f32) -> (usize, usize) {
    rig.live.write_input("seq", "clock", level).unwrap();
    counted_block(rig)
}

#[test]
fn a_pattern_written_while_it_plays_applies_from_the_next_block() {
    let mut rig = Rig::new(SEQUENCER);
    assert_eq!(clocked_block(&mut rig, 1.0), (0, 0));
    assert_eq!(frequency(&rig), hz(48));
    assert_eq!(clocked_block(&mut rig, 0.0), (0, 0));

    write(
        &rig,
        "seq",
        "pattern",
        r#"[{"note": 12}, {"note": 5}]"#.into(),
    );
    let shown = r#"[{"note":12},{"note":5}]"#;
    assert_eq!(read(&rig, "seq", "pattern"), shown.into());
    assert_eq!(clocked_block(&mut rig, 1.0), (0, 0));
    assert_eq!(frequency(&rig), hz(53), "step 1 of the new pattern");
    let ((), _, frees) = crate::alloc_counter::allocator_events(|| {
        rig.live.reclaim();
    });
    assert!(
        frees >= 2,
        "the reclaimer frees the replaced pattern: {frees}"
    );
    assert_eq!(clocked_block(&mut rig, 0.0), (0, 0));
    assert_eq!(clocked_block(&mut rig, 1.0), (0, 0));
    assert_eq!(frequency(&rig), hz(60));
    assert_eq!(read(&rig, "seq", "pattern"), shown.into());
}

#[test]
fn clock_edges_render_while_a_control_thread_writes_patterns() {
    let mut rig = Rig::new(SEQUENCER);
    let surface = rig.live.control_surfaces.lock().unwrap()["seq"].clone();
    let stop = AtomicBool::new(false);
    let written = AtomicUsize::new(0);
    let last = thread::scope(|scope| {
        let writer = scope.spawn(|| {
            let mut last = String::new();
            for n in 0u32.. {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let steps: Vec<_> = (0..1 + n % 16)
                    .map(|k| format!(r#"{{"note": {k}}}"#))
                    .collect();
                let pattern = format!("[{}]", steps.join(","));
                if surface
                    .set_control("pattern", pattern.as_str().into())
                    .is_ok()
                {
                    written.fetch_add(1, Ordering::Relaxed);
                    last = pattern;
                }
                thread::yield_now();
            }
            last
        });
        let mut events = (0, 0);
        // At least 400 blocks, and until 50 writes have landed meanwhile.
        let mut block = 0;
        while block < 400 || written.load(Ordering::Relaxed) < 50 {
            assert!(block < 10_000_000, "the writer stalled");
            block += 1;
            let (allocs, frees) = clocked_block(&mut rig, (block % 2) as f32);
            events = (events.0 + allocs, events.1 + frees);
            rig.live.reclaim();
        }
        rig.live.write_input("seq", "clock", 1.0).unwrap();
        assert_eq!(block_with_locks_held(&mut rig, &[]), Ok((0, 0)));
        assert_eq!(events, (0, 0));
        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap()
    });
    assert!(written.load(Ordering::Relaxed) >= 50);
    rig.render(1);
    let ControlValue::String(shown) = read(&rig, "seq", "pattern") else {
        panic!("text");
    };
    let steps = |text: &str| text.matches("note").count();
    assert_eq!(steps(&shown), steps(&last), "reads report the last write");
}

#[test]
fn a_development_exposing_pattern_hands_it_to_its_sequencer() {
    let mut rig = Rig::new(
        r#"{ "version": "1.0.0",
        "developments": [{ "name": "phrase", "definition": { "version": "1.0.0",
            "modules": [{ "id": "seq", "type": "step_sequencer",
                "config": { "step_count": 2, "pattern": [{ "note": 0 }] } }],
            "connections": [],
            "inputs": [{ "name": "clock", "to": "seq", "to_port": "clock" }],
            "outputs": [{ "name": "frequency", "from": "seq", "from_port": "frequency" }],
            "controls": [{ "name": "notes", "module": "seq", "control": "pattern" }] } }],
        "modules": [{ "id": "p", "type": "phrase", "config": { "notes": "[{\"note\": 2}]" } }],
        "connections": [] }"#,
    );
    let heard = |rig: &mut Rig, level: f32| {
        rig.live.write_input("p", "clock", level).unwrap();
        let events = counted_block(rig);
        let module = rig.graph.modules["p"].module();
        (events, module.get_output("frequency").unwrap())
    };
    assert_eq!(read(&rig, "p", "notes"), r#"[{"note":2}]"#.into());
    assert_eq!(heard(&mut rig, 1.0), ((0, 0), hz(50)), "from its config");
    heard(&mut rig, 0.0);
    write(&rig, "p", "notes", r#"[{"note": 7}, {"note": 9}]"#.into());
    assert_eq!(
        read(&rig, "p", "notes"),
        r#"[{"note":7},{"note":9}]"#.into()
    );
    assert_eq!(heard(&mut rig, 1.0), ((0, 0), hz(57)));
    let surface = rig.live.control_surfaces.lock().unwrap()["p"].clone();
    assert!(surface.set_control("notes", "[{".into()).is_err());
    assert_eq!(
        read(&rig, "p", "notes"),
        r#"[{"note":7},{"note":9}]"#.into()
    );
}
