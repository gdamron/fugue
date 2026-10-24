//! The SamplePlayer's declared controls.
//!
//! Two tables, one per mode: an elastic player also declares `time_ratio`,
//! last, so every other control keeps its index in both, and its `asset`
//! writes are analysed for elastic playback as they load.

use crate::control_request::{
    ControlContext, ControlDecl, ControlIndex, ControlTable, DeclKind, PayloadCodec, Prepared,
    RtValue,
};
use crate::modules::sample_loading::AssetLoader;
use crate::payload::Payload;
use crate::ControlValue;

pub(super) const ASSET: ControlIndex = ControlIndex(0);
pub(super) const PLAY: ControlIndex = ControlIndex(1);
pub(super) const PLAYING: ControlIndex = ControlIndex(2);
pub(super) const LOOP: ControlIndex = ControlIndex(3);
pub(super) const PITCH_RATIO: ControlIndex = ControlIndex(4);
pub(super) const TIME_RATIO: ControlIndex = ControlIndex(5);

/// The smallest ratio held: the read head always moves forward.
pub(super) const MIN_RATIO: f32 = 1e-4;

const ASSET_DESCRIPTION: &str = "Audio sample path, https URL, or package ref like \
     'fugue.drums.808@1.2.0:kick/long.wav' (WAV or FLAC)";

const fn asset(codec: PayloadCodec) -> ControlDecl {
    ControlDecl::payload("asset", codec, ASSET_DESCRIPTION).loads_on_write()
}

const PLAY_DECL: ControlDecl = ControlDecl::new(
    "play",
    DeclKind::Bool,
    RtValue::Bool(false),
    "true starts the sample from its beginning (again, if playing); false stops it",
)
.event();

const PLAYING_DECL: ControlDecl = ControlDecl::new(
    "playing",
    DeclKind::Bool,
    RtValue::Bool(false),
    "Whether the sample is playing",
)
.telemetry();

const LOOP_DECL: ControlDecl = ControlDecl::new(
    "loop",
    DeclKind::Bool,
    RtValue::Bool(false),
    "Loop playback when enabled",
);

const fn ratio(key: &'static str, description: &'static str) -> ControlDecl {
    ControlDecl::new(
        key,
        DeclKind::Number {
            min: 0.25,
            max: 4.0,
        },
        RtValue::F32(1.0),
        description,
    )
    .clamped(MIN_RATIO, f32::MAX)
}

const CLASSIC: &[ControlDecl] = &[
    asset(SAMPLE),
    PLAY_DECL,
    PLAYING_DECL,
    LOOP_DECL,
    ratio(
        "pitch_ratio",
        "Playback speed / pitch ratio (1.0 = native, 2.0 = up an octave)",
    ),
];

const ELASTIC: &[ControlDecl] = &[
    asset(ELASTIC_SAMPLE),
    PLAY_DECL,
    PLAYING_DECL,
    LOOP_DECL,
    ratio(
        "pitch_ratio",
        "Pitch ratio (1.0 = native, 2.0 = up an octave) without changing speed",
    ),
    ratio(
        "time_ratio",
        "Playback speed ratio (1.0 = native, 2.0 = double speed) without changing pitch",
    ),
];

pub(super) static CLASSIC_TABLE: ControlTable = ControlTable::of(CLASSIC);
pub(super) static ELASTIC_TABLE: ControlTable = ControlTable::of(ELASTIC);

/// The table of a player built in elastic mode, or in classic.
pub(super) fn table(elastic: bool) -> &'static ControlTable {
    if elastic {
        &ELASTIC_TABLE
    } else {
        &CLASSIC_TABLE
    }
}

/// The table's defaults, in index order.
pub(super) fn defaults(elastic: bool) -> impl Iterator<Item = RtValue> {
    let decls = if elastic { ELASTIC } else { CLASSIC };
    decls.iter().map(|decl| decl.default)
}

/// A classic player's sample: loaded at the engine rate.
const SAMPLE: PayloadCodec = PayloadCodec {
    prepare: prepare_classic,
};

/// An elastic player's sample: loaded, then analysed for elastic playback,
/// so the audio thread finds the analysis ready.
const ELASTIC_SAMPLE: PayloadCodec = PayloadCodec {
    prepare: prepare_elastic,
};

fn prepare_classic(context: &ControlContext<'_>, value: &ControlValue) -> Prepared {
    prepare(context, value, false)
}

fn prepare_elastic(context: &ControlContext<'_>, value: &ControlValue) -> Prepared {
    prepare(context, value, true)
}

fn prepare(context: &ControlContext<'_>, value: &ControlValue, elastic: bool) -> Prepared {
    let sample = context.module::<AssetLoader>()?.load(value.as_string()?)?;
    if elastic {
        sample.elastic_analysis();
    }
    Ok((Payload::from_arc(sample), value.clone()))
}
