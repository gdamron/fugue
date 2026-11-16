//! With a running clock the bridged gate is unchanged: a note and its held
//! steps are one unbroken gate, through a held edge up to 2D + 2 late.

use fugue::modules::{CellSequencer, Clock, Step};
use fugue::{Module, RenderEngine};
use serde_json::json;

use crate::release::cell;
use crate::rig::{assert_level, drive, rises, sequencer, D};

/// An invention playing a note and four held steps (five steps, longer
/// than the stopped-clock release) on a 240 BPM clock at 48 kHz, for three
/// loops: each pass is one gate, rising once, high from the note's edge to
/// the end of the last held step, then low over the rests.
#[test]
fn a_running_clock_keeps_a_long_held_note_one_gate_in_an_invention() {
    let cells = json!([[
        { "note": null },
        { "note": 0 },
        { "held": true },
        { "held": true },
        { "held": true },
        { "held": true },
        { "note": null },
        { "note": null },
    ]]);
    let document = json!({
        "version": "1.0.0",
        "modules": [
            { "id": "clock", "type": "clock", "config": { "bpm": 240.0 } },
            { "id": "seq", "type": "cell_sequencer",
              "config": { "root_note": 60, "step_count": 8, "cells": cells } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } },
        ],
        "connections": [
            { "from": "clock", "from_port": "beat", "to": "seq", "to_port": "clock" },
            { "from": "seq", "from_port": "gate", "to": "dac", "to_port": "audio_left" },
            { "from": "clock", "from_port": "beat", "to": "dac", "to_port": "audio_right" },
        ],
    });
    let mut engine = RenderEngine::new(48_000);
    engine
        .load_json(&document.to_string())
        .expect("the invention loads");
    let frames = 25 * 12_000;
    let (mut gate, mut beat) = (Vec::new(), Vec::new());
    let mut block = vec![0.0f32; 1024];
    while gate.len() < frames {
        engine.render_interleaved(&mut block).unwrap();
        for &[l, r] in block.as_chunks::<2>().0 {
            gate.push(l);
            beat.push(r);
        }
    }
    let edges = rises(&beat);
    for pass in 0..3 {
        let (note, end, next) = (
            edges[8 * pass + 1],
            edges[8 * pass + 6],
            edges[8 * pass + 9],
        );
        let what = format!("pass {pass}");
        assert_level(&gate, note + 5..end - 5, true, &what);
        let attacks = rises(&gate[note - 5..end - 5]).len();
        assert_eq!(
            attacks, 1,
            "{what}: one attack for the note and its held steps"
        );
        assert_level(
            &gate,
            end + 5..next - 5,
            false,
            &format!("{what}: the rests"),
        );
    }
}

/// The tempo halves (or a little slower) just after the note's edge: the
/// held edge comes 2D, 2D + 1 or 2D + 2 after the last edge and the gate
/// stays high into it, as one gate.
#[test]
fn a_held_edge_up_to_two_steps_and_two_samples_late_keeps_the_gate() {
    for late in [2 * D, 2 * D + 1, 2 * D + 2] {
        let mut seq = sequencer(cell(1, Step::note(0), 2), false);
        let held = D + late;
        let trace = drive(&mut seq, &[0, D, held, held + D], held + 3 * D);
        let what = format!("a held edge {late} samples after the note's");
        assert_level(&trace.gate, D..held + D, true, &what);
        assert_eq!(rises(&trace.gate), vec![D], "{what}: one attack");
    }
}

/// A 97 BPM clock (sixteenths of 7,422.68 samples at 48 kHz) halves its
/// tempo just after the note's edge, so the held edge lands two measured
/// steps plus two samples later; the gate holds through it.
#[test]
fn a_fractional_tempo_halving_keeps_the_held_gate() {
    let mut clock = Clock::new(48_000, 97.0);
    let mut pattern = vec![Step::rest(); 16];
    pattern[13] = Step::note(0);
    pattern[14] = Step::held();
    let mut seq = CellSequencer::new(48_000)
        .with_step_count(16)
        .with_cells(vec![pattern]);
    let (mut last_beat, mut edges, mut gate) = (0.0, Vec::new(), Vec::new());
    for n in 0..120_000 {
        clock.process(1);
        let beat = clock.get_output("beat_x4").unwrap();
        if beat > 0.5 && last_beat <= 0.5 {
            edges.push(n);
            if edges.len() == 14 {
                clock.set_control("bpm", 48.5).unwrap();
            }
        }
        last_beat = beat;
        seq.set_input("clock", beat).unwrap();
        seq.process(1);
        gate.push(seq.get_output("gate").unwrap());
    }
    let (note, held) = (edges[13], edges[14]);
    assert_eq!(held - note, 2 * (note - edges[12]) + 2, "edges {edges:?}");
    assert_level(
        &gate,
        note..held + 100,
        true,
        "through the halved held edge",
    );
}
