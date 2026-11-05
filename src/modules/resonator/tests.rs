use super::*;
use crate::control_request::ControlCells;

const RATE: u32 = 48_000;
const HOST: usize = 256;
const SHAPES: [&str; 4] = ["structure", "brightness", "damping", "position"];

/// A resonator with every control at its default and nothing connected
/// but `strum`.
pub(super) fn resonator() -> Resonator {
    let mut r = Resonator::with_cells(RATE, Arc::new(ControlCells::new(controls::defaults())));
    r.set_input_connected(STRUM_IN, true);
    r
}

/// Renders `blocks` host blocks, striking at the start of the first when
/// `strike`, and returns the mono sum.
fn render(r: &mut Resonator, blocks: usize, strike: bool) -> Vec<f32> {
    let mut out = Vec::with_capacity(blocks * HOST);
    for b in 0..blocks {
        r.inputs[STRUM_IN][..HOST].fill(if strike && b == 0 { 1.0 } else { 0.0 });
        r.process(HOST);
        out.extend_from_slice(&r.audio[..HOST]);
    }
    out
}

fn shape(patch: &Patch, key: &str) -> f32 {
    match key {
        "structure" => patch.structure,
        "brightness" => patch.brightness,
        "damping" => patch.damping,
        _ => patch.position,
    }
}

/// Writes `key` from 0 to 1 in one go while ringing, and returns the
/// largest change the voices see between neighbouring engine blocks.
fn largest_block_step(r: &mut Resonator, key: &str) -> f32 {
    r.set_control(key, 0.0).unwrap();
    for _ in 0..40 {
        r.next_patch();
    }
    r.set_control(key, 1.0).unwrap();
    let mut last = 0.0f32;
    let mut largest = 0.0f32;
    for _ in 0..40 {
        let value = shape(&r.next_patch(), key);
        largest = largest.max((value - last).abs());
        last = value;
    }
    assert_eq!(last, 1.0, "{key} arrives");
    largest
}

/// A jump of a shape control reaches the voices as a glide of about 10 ms
/// (20 engine blocks at 48 kHz), so a fast sweep has no block-edge steps.
/// The same measurement on a resonator whose glides are switched off sees
/// the whole jump in one block: the check fails without smoothing.
#[test]
fn shape_writes_glide_instead_of_stepping() {
    for key in SHAPES {
        let mut r = resonator();
        render(&mut r, 1, true);
        let step = largest_block_step(&mut r, key);
        assert!(step <= 1.0 / 19.0, "{key} steps {step} in one block");

        let mut raw = resonator();
        render(&mut raw, 1, true);
        for ramp in [
            &mut raw.structure,
            &mut raw.brightness,
            &mut raw.damping,
            &mut raw.position,
        ] {
            *ramp = LinearRamp::new(ramp.value(), 1);
        }
        let step = largest_block_step(&mut raw, key);
        assert!(step > 0.99, "unsmoothed {key} must step, saw {step}");
    }
}

/// Before anything is heard, a config value or early write lands at once,
/// so the first strike already has the configured shape.
#[test]
fn writes_before_the_first_block_land_at_once() {
    let mut r = resonator();
    r.set_control("brightness", 0.9).unwrap();
    assert_eq!(r.next_patch().brightness, 0.9);
}

/// A strum late in an engine block, with the pitch changing at the same
/// frame, starts the new voice at the new pitch, not an average with the
/// old one.
#[test]
fn a_strum_takes_the_pitch_that_came_with_it() {
    let mut r = resonator();
    r.set_input_connected(FREQUENCY_IN, true);
    r.inputs[FREQUENCY_IN][..ENGINE_BLOCK].fill(110.0);
    r.inputs[FREQUENCY_IN][20..ENGINE_BLOCK].fill(880.0);
    r.inputs[STRUM_IN][..ENGINE_BLOCK].fill(0.0);
    r.inputs[STRUM_IN][20..ENGINE_BLOCK].fill(1.0);
    r.process(ENGINE_BLOCK);
    assert_eq!(r.frequency_frames, 0, "the block was rendered");
    // Gather the next block at 880 Hz: the voice was struck at 880 Hz.
    let hz = r.voices.frequency(0) * RATE as f32;
    assert!((hz - 880.0).abs() < 0.01, "struck at {hz} Hz");
}

/// A struck voice that falls quiet stops running and its output is exact
/// silence, not a subnormal tail. (The sympathetic halo rings for at least
/// its own long damping, so it is left out.)
#[test]
fn a_quiet_voice_falls_to_exact_silence() {
    let models = MODELS.iter().enumerate();
    for (model, _) in models.filter(|(_, name)| **name != "sympathetic_strings") {
        let mut r = resonator();
        r.set_control("model", model as f32).unwrap();
        r.set_control("damping", 0.0).unwrap();
        let x = render(&mut r, 2 * RATE as usize / HOST, true);
        assert!(x.iter().any(|s| s.abs() > 1e-3), "model {model} sounds");
        let tail = &x[x.len() - HOST..];
        assert!(
            tail.iter().all(|s| *s == 0.0),
            "model {model} tail {:?}",
            &tail[..4]
        );
    }
}

/// The outputs never leave ±1, even when a loud input drives every model.
#[test]
fn a_hot_input_stays_within_full_scale() {
    for model in 0..MODELS.len() {
        let mut r = resonator();
        r.set_control("model", model as f32).unwrap();
        r.set_input_connected(AUDIO_IN, true);
        for b in 0..200 {
            for i in 0..HOST {
                r.inputs[AUDIO_IN][i] = if (b * HOST + i) % 97 < 3 { 8.0 } else { -8.0 };
            }
            r.process(HOST);
            for out in [&r.odd, &r.even, &r.audio] {
                assert!(out[..HOST].iter().all(|s| s.abs() <= 1.0 && s.is_finite()));
            }
        }
    }
}

/// Cost per frame with four voices ringing, for each model. Run with
/// `cargo test --release resonator_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn resonator_cost() {
    for (model, name) in MODELS.iter().enumerate() {
        let mut r = resonator();
        r.set_control("model", model as f32).unwrap();
        r.set_control("polyphony", 4.0).unwrap();
        r.set_control("damping", 0.9).unwrap();
        for _ in 0..4 {
            render(&mut r, 8, true);
        }
        let blocks = 10 * RATE as usize / HOST;
        let start = std::time::Instant::now();
        let x = render(&mut r, blocks, false);
        let ns = start.elapsed().as_nanos() as f64 / (blocks * HOST) as f64;
        assert!(x.iter().any(|s| s.abs() > 1e-4));
        println!("{name}: {ns:.0} ns/frame at polyphony 4");
    }
}
