//! The six-operator engine's voice handling: alternation, drones, patch
//! changes and the brightness slew.

use super::engine::SixOpEngine;
use super::patches::{PATCHES, PATCH_COUNT};
use super::voice::scrub_length;
use crate::modules::macro_oscillator::engine::{EngineParameters, ENGINE_BLOCK};

const SR: f32 = 48_000.0;

fn params(patch: usize) -> EngineParameters {
    let harmonics = (patch as f32 + 0.5) / PATCH_COUNT as f32;
    EngineParameters {
        note: 60.0,
        sample_period: 1.0 / SR,
        harmonics,
        harmonics_target: harmonics,
        timbre: 0.5,
        morph: 0.5,
        accent: 0.8,
        trigger: false,
        untriggered: false,
        trigger_held: false,
        key: 0,
        scale: 0,
    }
}

/// Renders `blocks` engine blocks with `p`, applying `each` before each
/// one, and returns the output.
fn run(
    engine: &mut SixOpEngine,
    p: &mut EngineParameters,
    blocks: usize,
    mut each: impl FnMut(usize, &mut EngineParameters),
) -> Vec<f32> {
    let mut all = Vec::with_capacity(blocks * ENGINE_BLOCK);
    let (mut out, mut aux) = ([0.0; ENGINE_BLOCK], [0.0; ENGINE_BLOCK]);
    for block in 0..blocks {
        each(block, p);
        engine.render(p, &mut out, &mut aux);
        assert_eq!(out, aux, "aux is the audio");
        all.extend_from_slice(&out);
    }
    all
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

fn blocks(seconds: f32) -> usize {
    (seconds * SR / ENGINE_BLOCK as f32) as usize
}

#[test]
fn a_triggered_engine_is_silent_until_struck() {
    let mut engine = SixOpEngine::new();
    let mut p = params(0);
    let quiet = run(&mut engine, &mut p, blocks(0.2), |_, _| {});
    assert!(quiet.iter().all(|v| *v == 0.0));
    let struck = run(&mut engine, &mut p, blocks(0.2), |b, p| p.trigger = b == 0);
    assert!(rms(&struck) > 0.01);
}

#[test]
fn the_last_note_rings_on_under_the_next() {
    // Two notes of a long bell: with two voices, the first still sounds
    // just after the second strikes, so the sum is louder than one note.
    let bell = 8;
    let mut one = SixOpEngine::new();
    let mut p = params(bell);
    run(&mut one, &mut p, blocks(0.3), |b, p| p.trigger = b == 0);
    let single = run(&mut one, &mut p, blocks(0.05), |_, _| {});

    let mut two = SixOpEngine::new();
    let mut p = params(bell);
    run(&mut two, &mut p, blocks(0.3), |b, p| p.trigger = b == 0);
    p.note = 67.0;
    let both = run(&mut two, &mut p, blocks(0.05), |b, p| p.trigger = b == 0);
    assert!(
        rms(&both) > rms(&single),
        "{} vs {}",
        rms(&both),
        rms(&single)
    );
}

#[test]
fn a_held_trigger_sustains_and_letting_go_releases() {
    let pad = 26;
    let render = |held: bool| {
        let mut engine = SixOpEngine::new();
        let mut p = params(pad);
        run(&mut engine, &mut p, blocks(1.5), |b, p| {
            p.trigger = b == 0;
            p.trigger_held = held;
        })
    };
    let held = render(true);
    let released = render(false);
    let tail = held.len() - blocks(0.2) * ENGINE_BLOCK;
    assert!(rms(&held[tail..]) > 4.0 * rms(&released[tail..]));
}

#[test]
fn a_drone_sounds_without_triggers_and_morph_scrubs_into_the_note() {
    let piano = 0;
    let level = |morph: f32| {
        let mut engine = SixOpEngine::new();
        let mut p = params(piano);
        p.untriggered = true;
        p.morph = morph;
        let x = run(&mut engine, &mut p, blocks(0.5), |_, _| {});
        rms(&x[x.len() / 2..])
    };
    let onset = level(0.0);
    let late = level(1.0);
    assert!(onset > 0.01, "{onset}");
    assert!(late < 0.3 * onset, "{late} vs {onset}");
}

#[test]
fn every_scrub_range_is_positive_and_bounded() {
    for patch in PATCHES.iter() {
        let t = scrub_length(patch);
        assert!(t > 0.0 && t <= 1.5, "{}: {t}", patch.name);
    }
}

#[test]
fn harmonics_holds_its_patch_near_a_band_edge() {
    // A note struck with harmonics just past patch 4's band, inside the
    // hysteresis, still plays patch 4; further past, patch 5.
    let strike = |target: f32| {
        let mut engine = SixOpEngine::new();
        let mut p = params(4);
        run(&mut engine, &mut p, 1, |_, _| {});
        run(&mut engine, &mut p, blocks(0.1), |b, p| {
            p.harmonics_target = target;
            p.trigger = b == 0;
        })
    };
    let band = 1.0 / PATCH_COUNT as f32;
    let reference = strike(4.5 * band);
    assert_eq!(strike(5.05 * band), reference);
    assert_ne!(strike(5.3 * band), reference);
}

/// The RMS of the difference between two renders.
fn distance(a: &[f32], b: &[f32]) -> f32 {
    let d: Vec<f32> = a.iter().zip(b).map(|(x, y)| x - y).collect();
    rms(&d)
}

#[test]
fn a_timbre_jump_reaches_the_modulators_gradually() {
    // Two drones in step, one left dark and one written bright: in the
    // first couple of milliseconds after the write they are still close,
    // and by a quarter of a second they are far apart.
    let render = |timbre: f32| {
        let mut engine = SixOpEngine::new();
        let mut p = params(16);
        p.untriggered = true;
        p.timbre = 0.0;
        run(&mut engine, &mut p, blocks(0.3), |_, _| {});
        p.timbre = timbre;
        run(&mut engine, &mut p, blocks(0.3), |_, _| {})
    };
    let dark = render(0.0);
    let bright = render(1.0);
    let first = 8 * ENGINE_BLOCK;
    let late = bright.len() - blocks(0.05) * ENGINE_BLOCK;
    let apart = distance(&dark[late..], &bright[late..]);
    let early = distance(&dark[..first], &bright[..first]);
    assert!(apart > 0.01, "{apart}");
    assert!(early < 0.25 * apart, "{early} vs {apart}");
}

/// The magnitude of `x` at `hz`, by a Goertzel filter.
fn magnitude(x: &[f32], hz: f32) -> f32 {
    let w = std::f32::consts::TAU * hz / SR;
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for v in x {
        let s = v + 2.0 * w.cos() * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    (s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2).sqrt() / x.len() as f32
}

#[test]
fn a_ringing_note_keeps_its_own_pitch() {
    // A3 then A4 on a long bell: after the second strike the first note
    // still rings at 220 Hz instead of jumping to the new pitch.
    let bell = 8;
    let strike = |notes: &[f32]| {
        let mut engine = SixOpEngine::new();
        let mut p = params(bell);
        let mut x = Vec::new();
        for note in notes {
            p.note = *note;
            x = run(&mut engine, &mut p, blocks(0.4), |b, p| p.trigger = b == 0);
        }
        x
    };
    let both = strike(&[57.0, 69.0]);
    let alone = strike(&[69.0]);
    let window = &both[blocks(0.1) * ENGINE_BLOCK..];
    let reference = &alone[blocks(0.1) * ENGINE_BLOCK..];
    assert!(
        magnitude(window, 220.0) > 4.0 * magnitude(reference, 220.0),
        "{} vs {}",
        magnitude(window, 220.0),
        magnitude(reference, 220.0)
    );
}

#[test]
fn a_drone_patch_change_crossfades_without_a_gap() {
    for (from, to) in [(24, 25), (0, 1), (16, 17)] {
        let mut engine = SixOpEngine::new();
        let mut p = params(from);
        p.untriggered = true;
        p.note = 57.0;
        let before = run(&mut engine, &mut p, blocks(0.5), |_, _| {});
        p.harmonics_target = (to as f32 + 0.5) / PATCH_COUNT as f32;
        let after = run(&mut engine, &mut p, blocks(0.5), |_, _| {});
        let window =
            |x: &[f32], from: f32, to: f32| rms(&x[(from * SR) as usize..(to * SR) as usize]);
        let quieter = window(&before, 0.4, 0.5).min(window(&after, 0.4, 0.5));
        let gap = window(&after, 0.02, 0.06);
        assert!(
            gap > 0.5 * quieter,
            "{from} to {to}: {gap} against {quieter}"
        );
    }
}
