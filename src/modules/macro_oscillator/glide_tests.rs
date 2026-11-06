//! Macro writes glide rather than step, in every model.

use super::*;
use serde_json::json;

const SR: u32 = 48_000;

fn render(module: &mut dyn Module, frames: usize, block: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames);
    while out.len() < frames {
        let n = block.min(frames - out.len());
        module.process(n);
        out.extend_from_slice(&module.output_block(0)[..n]);
    }
    out
}

/// A macro oscillator built from `config`, as its concrete type.
fn concrete(config: serde_json::Value) -> MacroOscillator {
    let cells = Arc::new(ControlCells::new(controls::defaults()));
    let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
    apply_control_keys(&surface, &config, |key| TABLE.resolve(key).is_some()).expect("config");
    MacroOscillator::with_cells(SR, cells)
}

#[test]
fn a_macro_write_glides_instead_of_stepping() {
    let blocks = macro_glide_blocks(SR) as usize;
    for (key, ramp) in [
        (
            "harmonics",
            (|m: &MacroOscillator| m.harmonics.value()) as fn(&MacroOscillator) -> f32,
        ),
        ("timbre", |m| m.timbre.value()),
        ("morph", |m| m.morph.value()),
    ] {
        let mut module = concrete(json!({ key: 0.5 }));
        // Whole engine blocks, so each `process` below renders exactly one.
        module.process(ENGINE_BLOCK * 4);
        module.set_control(key, 1.0).unwrap();
        // The value each engine block is rendered with.
        let mut seen = Vec::new();
        for _ in 0..blocks + 4 {
            module.process(ENGINE_BLOCK);
            seen.push(ramp(&module));
        }
        let most = 0.5 / blocks as f32 + 1e-6;
        let mut last = 0.5;
        for (block, value) in seen.iter().enumerate() {
            assert!(
                *value >= last && *value - last <= most,
                "{key} moved {} in engine block {block}",
                value - last
            );
            last = *value;
        }
        assert_eq!(seen[blocks - 1], 1.0, "{key} lands in {blocks} blocks");
        assert!(seen[blocks / 2] < 0.8, "{key} is still gliding halfway");
    }
}

#[test]
fn macros_written_before_the_first_block_start_there() {
    let mut module = concrete(json!({ "harmonics": 0.9, "timbre": 0.1 }));
    module.set_control("morph", 0.2).unwrap();
    module.process(ENGINE_BLOCK);
    assert_eq!(module.harmonics.value(), 0.9);
    assert_eq!(module.timbre.value(), 0.1);
    assert_eq!(module.morph.value(), 0.2);
}

/// The largest second difference in `x`: how sharply the waveform turns.
fn sharpest_turn(x: &[f32]) -> f32 {
    x.windows(3)
        .map(|w| (w[2] - 2.0 * w[1] + w[0]).abs())
        .fold(0.0, f32::max)
}

/// Every model spreads a block's parameters across the block's frames, so
/// a macro write never turns the waveform more sharply than the steady
/// sound on either side of it does. This catches a step in a model with a
/// smooth waveform (waveshaping fails it without the glide); in one with
/// hard edges (saws, sync) the edges set the bound, so it is a smoke test
/// there and the glide itself is checked above.
#[test]
fn macro_writes_do_not_put_corners_in_any_model() {
    const HOST_BLOCK: usize = 512;
    const SETTLE: usize = 48 * HOST_BLOCK;
    for model in 0..MODEL_COUNT {
        for key in ["harmonics", "timbre", "morph"] {
            // Wavetable's harmonics passes through the sync and bit-reduced
            // banks on its way from 0.1 to 0.9, whose edges are sharper
            // than either end: the wavetable tests check its block edges.
            if engines::MODELS[model] == "wavetable" && key == "harmonics" {
                continue;
            }
            // Six-operator FM's harmonics picks another patch, crossfading
            // to a different sound whose own detuned beating sets its
            // sharpest turn: the engine tests check that crossfade.
            if engines::MODELS[model] == "six_op_fm" && key == "harmonics" {
                continue;
            }
            let mut module = concrete(json!({
                "model": engines::MODELS[model], "frequency": 110.0, key: 0.1,
            }));
            let before = render(&mut module, SETTLE, HOST_BLOCK);
            module.set_control(key, 0.9).unwrap();
            let after = render(&mut module, 2 * SETTLE, HOST_BLOCK);
            let steady = sharpest_turn(&before[SETTLE / 2..]).max(sharpest_turn(&after[SETTLE..]));
            // The write arrives at the next engine block, 12 frames late.
            let change = sharpest_turn(&after[..4 * HOST_BLOCK]);
            assert!(
                change <= 1.25 * steady + 1e-4,
                "{}: {key} write turns {change} against a steady {steady}",
                engines::MODELS[model]
            );
        }
    }
}
