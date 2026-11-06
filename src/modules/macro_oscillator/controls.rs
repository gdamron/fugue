//! The macro oscillator's declared controls.

use super::engines::MODELS;
use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};
use crate::music::scales::{KEY_NAMES, KEY_SPELLINGS, SCALE_NAMES};

pub(super) const MODEL: ControlIndex = ControlIndex(0);
pub(super) const FREQUENCY: ControlIndex = ControlIndex(1);
pub(super) const FREQUENCY_MOD_DEPTH: ControlIndex = ControlIndex(2);
pub(super) const HARMONICS: ControlIndex = ControlIndex(3);
pub(super) const TIMBRE: ControlIndex = ControlIndex(4);
pub(super) const MORPH: ControlIndex = ControlIndex(5);
pub(super) const LEVEL: ControlIndex = ControlIndex(6);
pub(super) const DECAY: ControlIndex = ControlIndex(7);
pub(super) const KEY: ControlIndex = ControlIndex(8);
pub(super) const SCALE: ControlIndex = ControlIndex(9);

/// The shortest and longest low-pass gate decay, in seconds to fall 60 dB.
pub(super) const MIN_DECAY: f32 = 0.1;
pub(super) const MAX_DECAY: f32 = 15.0;

/// A 0-to-1 macro control.
const fn macro_control(key: &'static str, description: &'static str) -> ControlDecl {
    ControlDecl::new(
        key,
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.5),
        description,
    )
    .precision(3)
    .clamped(0.0, 1.0)
}

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "model",
        DeclKind::Choice(MODELS),
        RtValue::U32(0),
        "Synthesis model; a change crossfades",
    ),
    ControlDecl::new(
        "frequency",
        DeclKind::Number {
            min: 20.0,
            max: 20000.0,
        },
        RtValue::F32(440.0),
        "Frequency in Hz",
    )
    .unit("Hz")
    .log()
    .precision(1)
    .clamped(0.0, f32::MAX),
    ControlDecl::new(
        "frequency_mod_depth",
        DeclKind::Number {
            min: 0.0,
            max: 1000.0,
        },
        RtValue::F32(0.0),
        "Frequency modulation depth in Hz",
    )
    .unit("Hz"),
    macro_control(
        "harmonics",
        "Harmonic content; virtual analog: second oscillator detune; waveshaping: shaper curve (0.5 linear); fm: modulator ratio (0.5 an octave); grain_formant: second formant ratio, bare window below 0.5; additive: spectral bumps; chords: chord (octave, fifth, sus2, sus4, triad, sixth, seventh, ninth, eleventh, six-nine); speech: formant shift, ±1 octave; wavetable: bank (harmonic, formant, sync, bit-reduced), gliding through them to about 0.43, then stepping back with timbre and morph quantized too from about 0.57; six_op_fm: patch, 32 in four groups of eight (keys, bells and mallets, basses and plucks, organs, pads and leads); string: stiffness (inharmonic below and above 0.5); modal: material and inharmonicity; bass_drum: attack pitch blip, then self-modulation, then overdrive; snare_drum: shell to snares",
    ),
    macro_control(
        "timbre",
        "Brightness; virtual analog: pulse width and sync; waveshaping: fold amount; fm: modulation index; grain_formant: formant frequency; additive: spectral peak position; chords: inversion (two octaves down to two up, root position at 0.5); speech: vocal register, low to high voice; wavetable: position along the bank, dark to bright; six_op_fm: modulator levels, about 12 dB darker to 12 dB brighter; string: brightness; modal: brightness; bass_drum: tone; snare_drum: upper shell modes (aux: pitch blip)",
    ),
    macro_control(
        "morph",
        "Waveform morph; virtual analog: saw to triangle; waveshaping: triangle to saw; fm: feedback (below 0.5 rough, above noisy); grain_formant: grain window; additive: narrows the spectral peak; chords: organ registrations, bright to hollow, to about 0.47, then the notes turn one by one to wavetable voices that scan soft to bright; speech: vowel; wavetable: the bank's second axis of waves; six_op_fm: envelope length, up to 8 times shorter to 8 times longer, or with trigger unpatched, how far into the note the drone holds; string: decay; modal: decay; bass_drum: decay; snare_drum: decay",
    ),
    ControlDecl::new(
        "level",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(1.0),
        "Output level, 0 to 1",
    )
    .clamped(0.0, 1.0),
    ControlDecl::new(
        "decay",
        DeclKind::Number {
            min: MIN_DECAY,
            max: MAX_DECAY,
        },
        RtValue::F32(1.0),
        "Time for the low-pass gate to fall 60 dB after a trigger, in seconds; six_op_fm shapes its own envelopes and ignores it, and string, modal and the drums shape their own decay with morph",
    )
    .unit("s")
    .log()
    .clamped(MIN_DECAY, MAX_DECAY),
    ControlDecl::new(
        "key",
        DeclKind::Choice(KEY_NAMES),
        RtValue::U32(0),
        "Chords: the key chords are built in",
    )
    .spellings(KEY_SPELLINGS),
    ControlDecl::new(
        "scale",
        DeclKind::Choice(SCALE_NAMES),
        RtValue::U32(0),
        "Chords: the scale chord notes are taken from",
    ),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// Each control's starting value, in table order.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
