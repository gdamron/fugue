//! The granular processor's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const MODE: ControlIndex = ControlIndex(0);
pub(super) const POSITION: ControlIndex = ControlIndex(1);
pub(super) const SIZE: ControlIndex = ControlIndex(2);
pub(super) const PITCH: ControlIndex = ControlIndex(3);
pub(super) const DENSITY: ControlIndex = ControlIndex(4);
pub(super) const TEXTURE: ControlIndex = ControlIndex(5);
pub(super) const FREEZE: ControlIndex = ControlIndex(6);
pub(super) const SPREAD: ControlIndex = ControlIndex(7);
pub(super) const FEEDBACK: ControlIndex = ControlIndex(8);
pub(super) const SYNC: ControlIndex = ControlIndex(9);

/// The furthest `pitch` transposes, in semitones either way.
pub(super) const MOST_PITCH: f32 = 24.0;

/// The playback modes, in [`super::playback::Mode`] order.
pub(super) const MODES: &[&str] = &["granular", "stretch"];

/// A 0-to-1 control.
const fn unit(key: &'static str, default: f32, description: &'static str) -> ControlDecl {
    ControlDecl::new(
        key,
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(default),
        description,
    )
    .precision(3)
    .clamped(0.0, 1.0)
}

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "mode",
        DeclKind::Choice(MODES),
        RtValue::U32(0),
        "Playback mode: granular plays grains from the buffer; stretch holds it still in time while its pitch moves; a change crossfades",
    ),
    unit(
        "position",
        0.5,
        "Where playback reads in the buffer: 0 the newest audio, 1 the oldest",
    ),
    unit(
        "size",
        0.5,
        "Granular: grain length, about 32 ms at 0 to 0.5 s at 1. Stretch: window length, 32 ms to 128 ms",
    ),
    ControlDecl::new(
        "pitch",
        DeclKind::Number {
            min: -MOST_PITCH,
            max: MOST_PITCH,
        },
        RtValue::F32(0.0),
        "Transposition of what plays back, in semitones",
    )
    .unit("st")
    .precision(2)
    .clamped(-MOST_PITCH, MOST_PITCH),
    unit(
        "density",
        0.7,
        "Granular: how many grains; below 0.5 at regular intervals (more towards 0), above at random times (more towards 1), none around 0.5. Other modes: how much the sound is smeared",
    ),
    unit(
        "texture",
        0.5,
        "Granular: grain window, near-rectangular at 0, triangular near 0.375, a smooth bell at 0.75; above 0.75 smears the grains. Other modes: tone, a low-pass below 0.5, a high-pass above",
    ),
    ControlDecl::new(
        "freeze",
        DeclKind::Bool,
        RtValue::Bool(false),
        "Stop recording and keep playing the buffer",
    ),
    unit(
        "spread",
        0.0,
        "Granular: random stereo placement of each grain, 0 all centred",
    ),
    unit(
        "feedback",
        0.0,
        "Share of the output fed back into the recording; off while frozen",
    ),
    ControlDecl::new(
        "sync",
        DeclKind::Bool,
        RtValue::Bool(false),
        "Granular: while the clock input is patched, grains start only on clock divisions (1 to 8 per pulse, set by density)",
    ),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// Each control's starting value, in table order.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}

/// Grains per clock pulse, as `density` moves away from its silent middle.
const DIVISIONS: [u32; 6] = [1, 2, 3, 4, 6, 8];

/// Grains per clock pulse for a `reach` (0 to 1) away from silence.
pub(super) fn divisions(reach: f32) -> u32 {
    let step = (reach.clamp(0.0, 1.0) * DIVISIONS.len() as f32) as usize;
    DIVISIONS[step.min(DIVISIONS.len() - 1)]
}
