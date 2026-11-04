//! The drum map's declared controls.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const X: ControlIndex = ControlIndex(0);
pub(super) const Y: ControlIndex = ControlIndex(1);
pub(super) const KICK_DENSITY: ControlIndex = ControlIndex(2);
pub(super) const SNARE_DENSITY: ControlIndex = ControlIndex(3);
pub(super) const HAT_DENSITY: ControlIndex = ControlIndex(4);
pub(super) const CHAOS: ControlIndex = ControlIndex(5);
pub(super) const SWING: ControlIndex = ControlIndex(6);
pub(super) const SEED: ControlIndex = ControlIndex(7);
pub(super) const RESOLUTION: ControlIndex = ControlIndex(8);
pub(super) const MODE: ControlIndex = ControlIndex(9);

/// The largest seed: integer controls stay exact in an `f32`.
const MAX_SEED: i32 = (1 << 24) - 1;

/// A 0-to-1 control.
const fn unit(key: &'static str, default: f32, description: &'static str) -> ControlDecl {
    ControlDecl::new(
        key,
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(default),
        description,
    )
    .clamped(0.0, 1.0)
}

const DECLS: &[ControlDecl] = &[
    unit(
        "x",
        0.5,
        "Map position across groove families: straight (0), syncopated (0.5), broken (1)",
    ),
    unit(
        "y",
        0.5,
        "Map position in energy: sparse half-time (0), standard (0.5), driving (1)",
    ),
    unit(
        "kick_density",
        0.5,
        "How much of the map's kick part plays: 0 is silent, 1 plays every step it allows",
    ),
    unit(
        "snare_density",
        0.5,
        "How much of the map's snare part plays: 0 is silent, 1 plays every step it allows",
    ),
    unit(
        "hat_density",
        0.5,
        "How much of the map's hat part plays: 0 is silent, 1 plays every step it allows",
    ),
    unit(
        "chaos",
        0.0,
        "Random variation of the pattern from bar to bar, drawn from seed",
    ),
    unit(
        "swing",
        0.0,
        "Delays the off-beat 16ths: 0 is straight, 2/3 a triplet feel, 1 a dotted feel",
    ),
    ControlDecl::new(
        "seed",
        DeclKind::Integer {
            min: 0,
            max: MAX_SEED,
        },
        RtValue::I32(0),
        "Picks the chaos variation: the same seed repeats the same bars",
    )
    .clamped(0.0, MAX_SEED as f32),
    ControlDecl::new(
        "resolution",
        DeclKind::Integer { min: 16, max: 32 },
        RtValue::I32(16),
        "Steps per bar: 16, or 32 to add the 32nd notes between clock edges",
    )
    .clamped(16.0, 32.0),
    ControlDecl::new(
        "mode",
        DeclKind::Choice(&["map", "euclidean"]),
        RtValue::U32(0),
        "map plays the pattern map; euclidean spreads round(density x steps) hits evenly",
    ),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// The table's defaults, in index order.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
