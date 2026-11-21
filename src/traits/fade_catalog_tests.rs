//! Every module's decision, input by input, on whether a cable patched in
//! ramps in (see [`crate::Module::input_fades_in`]). The default goes by
//! name, which is only a guess: a module whose audio input has another
//! name, or whose control input starts with `audio`, fails here and is
//! decided on purpose.

use crate::module_config::tests::registry::{base_config, UNBUILDABLE};
use crate::ModuleRegistry;

/// Each module type, the inputs that fade in and the inputs that take a
/// new cable at once, as its base config builds it. A `code` module's
/// inputs are its script's, and a `wasm_module`'s its guest's (it is left
/// out, built only from a compiled guest): both are decided by name.
const DECIDED: &[(&str, &[&str], &[&str])] = &[
    (
        "adsr",
        &[],
        &["gate", "attack", "decay", "sustain", "release"],
    ),
    ("agent", &[], &["trigger", "reset"]),
    (
        "audio_file_sink",
        &["audio", "audio_left", "audio_right"],
        &[],
    ),
    (
        "cell_sequencer",
        &[],
        &[
            "clock",
            "reset",
            "next_cell",
            "previous_cell",
            "select_cell",
            "wait_for_cycle_end",
        ],
    ),
    ("chance", &[], &["clock", "reset"]),
    ("clock", &[], &[]),
    ("code", &[], &[]),
    (
        "contour",
        &[],
        &[
            "trigger",
            "clock",
            "frequency",
            "frequency_mod",
            "shape_mod",
            "slope_mod",
            "smoothness_mod",
        ],
    ),
    ("control_scheduler", &[], &["clock", "reset"]),
    ("dac", &["audio", "audio_left", "audio_right"], &[]),
    ("delay", &["audio"], &["time_mod", "bpm"]),
    ("divisi", &[], &["frequency", "gate", "velocity"]),
    ("drive", &["audio"], &[]),
    ("drum_map", &[], &["clock", "reset"]),
    ("filter", &["audio"], &["cutoff", "cutoff_mod", "resonance"]),
    ("follower", &["audio_left", "audio_right", "sidechain"], &[]),
    ("gain", &["audio"], &["decibels_mod"]),
    ("gain_curve", &[], &["level"]),
    (
        "granular",
        &["audio_left"],
        &["audio_right", "trigger", "clock"],
    ),
    ("lfo", &[], &["sync", "rate_mod", "bpm"]),
    ("limiter", &["audio_left", "audio_right"], &[]),
    (
        "macro_oscillator",
        &[],
        &["frequency", "frequency_mod", "trigger", "level"],
    ),
    ("melody", &[], &["gate"]),
    (
        "mixer",
        &["audio.0", "audio.1", "audio.2", "audio.3"],
        &[
            "level.0", "level.1", "level.2", "level.3", "pan.0", "pan.1", "pan.2", "pan.3",
            "master",
        ],
    ),
    ("noise", &[], &[]),
    (
        "oscillator",
        &["amplitude_mod"],
        &["frequency", "frequency_mod"],
    ),
    ("phaser", &["audio_left"], &["audio_right", "bpm"]),
    ("resonator", &["audio"], &["frequency", "strum"]),
    ("reverb", &["audio_left"], &["audio_right"]),
    ("rtmp_sink", &["audio", "audio_left", "audio_right"], &[]),
    ("sample_instrument", &[], &["frequency", "gate", "velocity"]),
    ("sample_kit", &[], &["play", "key"]),
    ("sample_player", &[], &["play", "loop", "pitch_ratio"]),
    ("sample_slicer", &[], &["play", "slice"]),
    ("step_sequencer", &[], &["clock", "reset"]),
    ("sustain", &[], &["gate", "pedal"]),
    ("vca", &["audio"], &["level"]),
    ("youtube_sink", &["audio", "audio_left", "audio_right"], &[]),
];

#[test]
fn every_module_input_fades_in_as_decided() {
    let registry = ModuleRegistry::default();
    let unbuildable = |type_id: &&str| UNBUILDABLE.iter().any(|(id, _)| id == type_id);
    let mut types: Vec<&str> = registry.types().filter(|t| !unbuildable(t)).collect();
    types.sort_unstable();
    let listed: Vec<&str> = DECIDED.iter().map(|(type_id, ..)| *type_id).collect();
    assert_eq!(types, listed, "decide each new module's inputs here");
    for (type_id, fades, steps) in DECIDED {
        let built = match registry.build(type_id, 48_000, &base_config(type_id)) {
            Ok(built) => built,
            // A streaming sink looks for its encoder as it builds: where
            // ffmpeg is missing (CI, say) it is checked elsewhere.
            Err(error) if registry.is_sink(type_id) && error.to_string().contains("ffmpeg") => {
                continue
            }
            Err(error) => panic!("{type_id}: {error}"),
        };
        let module = built.module.module();
        let inputs = module.inputs();
        let (mut fading, mut stepping) = (Vec::new(), Vec::new());
        for (index, name) in inputs.iter().enumerate() {
            if module.input_fades_in(index) {
                fading.push(*name);
            } else {
                stepping.push(*name);
            }
        }
        assert_eq!((&fading[..], &stepping[..]), (*fades, *steps), "{type_id}");
    }
}
