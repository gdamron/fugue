//! The wavetable model and the chords wavetable half, through the module.

use super::*;
use serde_json::json;

const SR: u32 = 48_000;
const HOST_BLOCK: usize = 64;

fn module(config: serde_json::Value) -> MacroOscillator {
    let cells = Arc::new(ControlCells::new(controls::defaults()));
    let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
    apply_control_keys(&surface, &config, |key| TABLE.resolve(key).is_some()).expect("config");
    MacroOscillator::with_cells(SR, cells)
}

/// `frames` of `audio` and `aux`, with `write` called before each host
/// block with the block's index.
fn render(
    m: &mut MacroOscillator,
    frames: usize,
    mut write: impl FnMut(&mut MacroOscillator, usize),
) -> (Vec<f32>, Vec<f32>) {
    let (mut audio, mut aux) = (Vec::new(), Vec::new());
    let mut block = 0;
    while audio.len() < frames {
        write(m, block);
        let n = HOST_BLOCK.min(frames - audio.len());
        m.process(n);
        audio.extend_from_slice(&m.output_block(0)[..n]);
        aux.extend_from_slice(&m.output_block(1)[..n]);
        block += 1;
    }
    (audio, aux)
}

/// The largest sample-to-sample step where host or engine blocks meet,
/// and anywhere else. End to end, through the module's glides: no edge
/// steps further than the waves' own corners do. (The engine's own test
/// holds the phase still to show the scan has no steps at all.)
fn edge_and_other_steps(x: &[f32]) -> (f32, f32) {
    let (mut edge, mut other) = (0.0f32, 0.0f32);
    for (i, w) in x.windows(2).enumerate() {
        let step = (w[1] - w[0]).abs();
        if (i + 1) % HOST_BLOCK == 0 || (i + 1) % ENGINE_BLOCK == 0 {
            edge = edge.max(step);
        } else {
            other = other.max(step);
        }
    }
    (edge, other)
}

#[test]
fn fast_wavetable_sweeps_do_not_step_at_block_edges() {
    // Each macro across its whole range and back in 20 ms each way: far
    // faster than a hand turns a knob.
    let sweep = SR as usize / 50;
    let blocks = sweep / HOST_BLOCK;
    for hz in [220.0, 1760.0] {
        for key in ["harmonics", "timbre", "morph"] {
            for other in [0.0, 0.5, 1.0] {
                let mut config = json!({
                    "model": "wavetable", "frequency": hz,
                    "harmonics": other, "timbre": other, "morph": other,
                });
                config[key] = json!(0.0);
                let mut m = module(config);
                render(&mut m, SR as usize / 10, |_, _| {});
                let (x, _) = render(&mut m, 2 * sweep, |m, block| {
                    let t = (block as f32 / blocks as f32).min(2.0);
                    let value = if t <= 1.0 { t } else { 2.0 - t };
                    m.set_control(key, value).unwrap();
                });
                let (edge, elsewhere) = edge_and_other_steps(&x);
                assert!(
                    edge <= 1.5 * elsewhere,
                    "{key} sweep at {hz} Hz (others {other}): edge step {edge} vs {elsewhere}"
                );
            }
        }
    }
}

#[test]
fn wavetable_aux_is_five_bit() {
    let mut m = module(json!({ "model": "wavetable", "frequency": 220.0, "timbre": 0.7 }));
    let (_, aux) = render(&mut m, SR as usize / 2, |_, _| {});
    let mut levels: Vec<i32> = aux[SR as usize / 10..]
        .iter()
        .map(|v| (v * 1e5).round() as i32)
        .collect();
    levels.sort_unstable();
    levels.dedup();
    assert!(levels.len() <= 65, "{} levels", levels.len());
    assert!(levels.len() >= 8, "{} levels", levels.len());
}

#[test]
fn wavetable_stays_bounded_under_wild_modulation() {
    let mut m = module(json!({ "model": "wavetable" }));
    let mut seed = 0x2545_f491_u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    let (audio, aux) = render(&mut m, SR as usize, |m, _| {
        m.set_control("harmonics", next()).unwrap();
        m.set_control("timbre", next()).unwrap();
        m.set_control("morph", next()).unwrap();
        m.set_control("frequency", 20.0 + 12_000.0 * next())
            .unwrap();
    });
    for s in audio.iter().chain(&aux) {
        assert!(s.is_finite() && s.abs() <= 1.0, "{s}");
    }
}

/// Cost per frame of the wavetable model. Run with `cargo test --release
/// wavetable_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn wavetable_cost() {
    for (what, harmonics) in [("gliding banks", 0.2), ("quantized", 0.8)] {
        let mut m = module(json!({
            "model": "wavetable", "frequency": 220.0,
            "harmonics": harmonics, "timbre": 0.4, "morph": 0.6,
        }));
        render(&mut m, SR as usize / 10, |_, _| {});
        let frames = 10 * SR as usize;
        let start = std::time::Instant::now();
        let (x, _) = render(&mut m, frames, |_, _| {});
        let ns = start.elapsed().as_nanos() as f64 / frames as f64;
        assert!(x.iter().any(|s| s.abs() > 1e-4));
        println!("wavetable ({what}): {ns:.0} ns/frame");
    }
}
