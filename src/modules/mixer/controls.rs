//! The mixer's declared controls: its channel count, read-only, its master,
//! and one level and one pan per channel. A level or pan exists only below
//! the channel count, which the config fixes for the mixer's life.

use crate::control_request::{ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue};

use super::MAX_CHANNELS;

// `channel_count` is index 0, set as the mixer is built (see `defaults`).
pub(super) const MASTER: ControlIndex = ControlIndex(1);
const FIRST_LEVEL: u16 = 2;
const FIRST_PAN: u16 = FIRST_LEVEL + MAX_CHANNELS as u16;

/// The level a channel starts at, unity gain, and its range.
pub(super) const DEFAULT_LEVEL: f32 = 1.0;
pub(super) const MAX_LEVEL: f32 = 2.0;
/// The pan a channel starts at, the centre.
pub(super) const DEFAULT_PAN: f32 = 0.0;

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "channel_count",
        DeclKind::Integer {
            min: 1,
            max: MAX_CHANNELS as i32,
        },
        RtValue::I32(4),
        "Read-only: how many channels the mixer has, as its config sets",
    )
    .telemetry(),
    ControlDecl::new(
        "master",
        DeclKind::Number {
            min: 0.0,
            max: MAX_LEVEL,
        },
        RtValue::F32(DEFAULT_LEVEL),
        "Master output level",
    )
    .clamped(0.0, MAX_LEVEL),
    ControlDecl::new(
        "level",
        DeclKind::Number {
            min: 0.0,
            max: MAX_LEVEL,
        },
        RtValue::F32(DEFAULT_LEVEL),
        "Channel level",
    )
    .clamped(0.0, MAX_LEVEL)
    .indexed(MAX_CHANNELS as u16)
    .listed_below("channel_count"),
    ControlDecl::new(
        "pan",
        DeclKind::Number {
            min: -1.0,
            max: 1.0,
        },
        RtValue::F32(DEFAULT_PAN),
        "Channel pan, from -1 (left) to 1 (right)",
    )
    .clamped(-1.0, 1.0)
    .indexed(MAX_CHANNELS as u16)
    .listed_below("channel_count"),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// The table's defaults, in index order, with `channels` as the count.
pub(super) fn defaults(channels: usize) -> impl Iterator<Item = RtValue> {
    let count = std::iter::once(RtValue::I32(channels as i32));
    let rest = DECLS[1..]
        .iter()
        .flat_map(|decl| std::iter::repeat_n(decl.default, usize::from(decl.count)));
    count.chain(rest)
}

/// Channel `channel`'s level control.
pub(super) fn level_index(channel: usize) -> ControlIndex {
    ControlIndex(FIRST_LEVEL + channel as u16)
}

/// Channel `channel`'s pan control.
pub(super) fn pan_index(channel: usize) -> ControlIndex {
    ControlIndex(FIRST_PAN + channel as u16)
}

/// A channel's control: its level or its pan.
pub(super) enum Channel {
    Level(usize),
    Pan(usize),
}

/// Which channel's level or pan `control` is, if either.
pub(super) fn channel(control: ControlIndex) -> Option<Channel> {
    let offset = usize::from(control.0).checked_sub(usize::from(FIRST_LEVEL))?;
    match offset {
        offset if offset < MAX_CHANNELS => Some(Channel::Level(offset)),
        offset if offset < 2 * MAX_CHANNELS => Some(Channel::Pan(offset - MAX_CHANNELS)),
        _ => None,
    }
}
