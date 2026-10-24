//! Live controls for `sample_slicer`, declared only in elastic mode.
//!
//! Classic slicing has no control surface: playback is fully determined by
//! the trigger/slice inputs. Elastic mode adds the shared time/pitch ratios
//! so slices can be tempo-fit without chipmunking.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

pub(super) const TIME_RATIO: ControlIndex = ControlIndex(0);
pub(super) const PITCH_RATIO: ControlIndex = ControlIndex(1);

/// The smallest ratio a write holds: a zero or negative ratio would stall
/// playback, so the read head always advances forward.
pub(super) const MIN_RATIO: f32 = 1e-4;

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "time_ratio",
        DeclKind::Number {
            min: 0.25,
            max: 4.0,
        },
        RtValue::F32(1.0),
        "Slice playback speed ratio (1.0 = native, 2.0 = double speed) \
         without changing pitch",
    )
    .clamped(MIN_RATIO, f32::MAX),
    ControlDecl::new(
        "pitch_ratio",
        DeclKind::Number {
            min: 0.25,
            max: 4.0,
        },
        RtValue::F32(1.0),
        "Slice pitch ratio (1.0 = native, 2.0 = up an octave) \
         without changing speed",
    )
    .clamped(MIN_RATIO, f32::MAX),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// The table's defaults, in index order: where every elastic slicer's cells
/// start.
pub(super) fn defaults() -> impl Iterator<Item = RtValue> {
    DECLS.iter().map(|decl| decl.default)
}
