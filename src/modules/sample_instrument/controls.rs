//! The SampleInstrument's controls, all declared, in a table built for each
//! instrument since its zone count is fixed at build: `release`,
//! `note_on`, `note_off`, and per zone `root_note.N`, `asset.N` and
//! `level.N`. A zone past the count is no control at all.
//!
//! - `release` is the release fade in seconds: positive, held within
//!   0.001..30.
//! - `note_on` and `note_off` are events naming a MIDI note (rounded,
//!   0..=127), each write a note. A control thread's notes are requests,
//!   applied in the order sent. A scheduler's are queued, so a chord at one
//!   step plays every note, with the last [`NOTE_OFF_RESERVE`] places kept
//!   for note offs: note ons crowding the queue cannot strand a note.
//! - `root_note.N` reads back the zone's root; it is fixed at build.
//! - `asset.N` is the zone's sample, loaded on a control thread at the
//!   engine rate with the zone's sustain loop scaled to it (`ZONE_AUDIO`),
//!   adopted by the instrument at a block boundary; the zone audio it
//!   replaces is retired off the audio thread.
//! - `level.N` is a number the instrument clamps to 0..2.

use std::sync::Arc;

use crate::control_request::{
    ControlCells, ControlContext, ControlDecl, ControlIndex, ControlTable, DeclKind, EventReserve,
    PayloadCodec, Prepared, RtValue,
};
use crate::invention::declared::DeclaredSurface;
use crate::modules::sample_loading::{AssetLoader, SampleData};
use crate::payload::{Payload, Shared};
use crate::ControlValue;

/// One keymap zone as authored in config: a sample, the note it was
/// recorded at, the key range it covers, and an optional sustain loop.
pub struct ZoneSpec {
    pub root_note: u8,
    pub key_low: u8,
    pub key_high: u8,
    pub asset: String,
    pub level: f32,
    pub loop_spec: Option<LoopSpec>,
}

/// A sustain loop authored in the source file's frame domain.
#[derive(Debug, Clone, Copy)]
pub struct LoopSpec {
    pub start_frames: u64,
    pub end_frames: u64,
    pub crossfade_frames: u64,
}

/// A sustain loop scaled to the engine rate at load time.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScaledLoop {
    pub start: f64,
    pub end: f64,
    pub crossfade: f64,
}

/// The buffer and loop a zone plays. Swapped wholesale when `asset.N`
/// re-points the zone, so a voice can never see a loop from one file
/// against the buffer of another.
pub(crate) struct ZoneAudio {
    /// The sample cache's own buffer, which it keeps while anything else
    /// holds it (see `load_cached_sample`), so a voice letting go of its
    /// clone on the audio thread frees nothing.
    pub sample: Arc<SampleData>,
    pub loop_region: Option<ScaledLoop>,
}

/// What a control index of an instrument's table names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InstrumentControl {
    Release,
    NoteOn,
    NoteOff,
    RootNote(usize),
    Asset(usize),
    Level(usize),
}

/// Controls before the zones' own.
const FIRST_ZONE_CONTROL: usize = 3;

/// The index of `control` in the table of an instrument of `zones` zones:
/// `release`, `note_on`, `note_off`, then each zone's `root_note`, `asset`
/// and `level` in turn.
pub(super) const fn index_of(control: InstrumentControl, zones: usize) -> ControlIndex {
    let index = match control {
        InstrumentControl::Release => 0,
        InstrumentControl::NoteOn => 1,
        InstrumentControl::NoteOff => 2,
        InstrumentControl::RootNote(zone) => FIRST_ZONE_CONTROL + zone,
        InstrumentControl::Asset(zone) => FIRST_ZONE_CONTROL + zones + zone,
        InstrumentControl::Level(zone) => FIRST_ZONE_CONTROL + 2 * zones + zone,
    };
    ControlIndex(index as u16)
}

/// What `index` names in the table of an instrument of `zones` zones.
pub(super) fn control_at(index: ControlIndex, zones: usize) -> Option<InstrumentControl> {
    let index = usize::from(index.0);
    match index {
        0 => return Some(InstrumentControl::Release),
        1 => return Some(InstrumentControl::NoteOn),
        2 => return Some(InstrumentControl::NoteOff),
        _ => {}
    }
    let index = index - FIRST_ZONE_CONTROL;
    let (group, zone) = (index / zones.max(1), index % zones.max(1));
    match group {
        _ if zone >= zones => None,
        0 => Some(InstrumentControl::RootNote(zone)),
        1 => Some(InstrumentControl::Asset(zone)),
        2 => Some(InstrumentControl::Level(zone)),
        _ => None,
    }
}

/// The most zones an instrument takes: one control index per zone control.
pub(super) const MAX_ZONES: usize = (u16::MAX as usize - FIRST_ZONE_CONTROL) / 3;

/// How many scheduled notes an instrument takes between two blocks.
pub(super) const NOTE_QUEUE: usize = 256;

/// Places at the end of the scheduled-note queue that only a note off may
/// take, so a schedule crowding it with note ons can still release them.
pub(super) const NOTE_OFF_RESERVE: usize = 16;

/// The release fade's range in seconds.
pub(super) const RELEASE_RANGE: (f32, f32) = (1e-3, 30.0);

/// Every zone's authored sustain loop, attached to the surface so an
/// `asset.N` write scales its zone's loop against the new file.
struct ZoneLoops(Vec<Option<LoopSpec>>);

/// The codec of a zone's `asset.N`: loads the sample through the attached
/// [`AssetLoader`] and scales the zone's loop to it. The control reads back
/// the value as written.
const ZONE_AUDIO: PayloadCodec = PayloadCodec {
    prepare: prepare_zone_audio,
};

fn prepare_zone_audio(context: &ControlContext<'_>, value: &ControlValue) -> Prepared {
    let ZoneLoops(loops) = context.module::<ZoneLoops>()?;
    let loop_spec = loops.get(usize::from(context.position)).copied().flatten();
    let loader = context.module::<AssetLoader>()?;
    let audio = zone_audio(loader, value.as_string()?, loop_spec.as_ref())?;
    Ok((Payload::new(audio), value.clone()))
}

const NOTE: DeclKind = DeclKind::Number {
    min: 0.0,
    max: 127.0,
};

/// The table of an instrument of `zones` zones, in [`index_of`] order.
fn table(zones: usize) -> ControlTable {
    let count = zones as u16;
    let decls = vec![
        ControlDecl::new(
            "release",
            DeclKind::Number {
                min: RELEASE_RANGE.0,
                max: RELEASE_RANGE.1,
            },
            RtValue::F32(0.1),
            "Release fade time in seconds after a note's gate falls",
        ),
        ControlDecl::new(
            "note_on",
            NOTE,
            RtValue::F32(0.0),
            "Start a note by MIDI note number (full velocity)",
        )
        .event(),
        ControlDecl::new(
            "note_off",
            NOTE,
            RtValue::F32(0.0),
            "Release a held note by MIDI note number",
        )
        .event(),
        ControlDecl::new(
            "root_note",
            DeclKind::Integer { min: 0, max: 127 },
            RtValue::I32(0),
            "Zone root note (fixed at build time)",
        )
        .telemetry()
        .indexed(count),
        ControlDecl::payload(
            "asset",
            ZONE_AUDIO,
            "Zone sample: path, https URL, or package ref like \
             'fugue.keys.grand@1.0.0:c4.wav' (WAV or FLAC)",
        )
        .loads_on_write()
        .indexed(count),
        ControlDecl::new(
            "level",
            DeclKind::Number { min: 0.0, max: 2.0 },
            RtValue::F32(1.0),
            "Zone level (1.0 = unity)",
        )
        .clamped(0.0, 2.0)
        .indexed(count),
    ];
    ControlTable::built(decls)
        .expect("an instrument's table is valid for every zone count it takes")
}

/// An instrument's controls as built: its table and cells, the surface
/// clients write, and each zone's audio, loaded at the engine rate.
pub(super) struct BuiltControls {
    pub(super) table: ControlTable,
    pub(super) cells: Arc<ControlCells>,
    pub(super) surface: DeclaredSurface,
    pub(super) audio: Vec<Shared<ZoneAudio>>,
}

/// Builds an instrument's controls from its zone specs, loading every
/// sample here (decode, resample and loop scaling happen on this thread,
/// under its build lock).
pub(super) fn build(
    sample_rate: u32,
    release: f32,
    specs: &[ZoneSpec],
) -> Result<BuiltControls, String> {
    let zones = specs.len();
    if zones > MAX_ZONES {
        return Err(format!(
            "an instrument takes at most {MAX_ZONES} zones, not {zones}"
        ));
    }
    let loader = AssetLoader::current(sample_rate);
    let mut audio = Vec::with_capacity(zones);
    for (index, spec) in specs.iter().enumerate() {
        let loaded = zone_audio(&loader, &spec.asset, spec.loop_spec.as_ref())
            .map_err(|err| format!("zones[{}]: {}", index, err))?;
        audio.push(Shared::new(loaded));
    }

    let table = table(zones);
    let roots = specs.iter().map(|spec| RtValue::I32(spec.root_note.into()));
    let unheld = std::iter::repeat_n(RtValue::Bool(false), zones);
    let levels = specs
        .iter()
        .map(|spec| RtValue::F32(spec.level.clamp(0.0, 2.0)));
    let values = [RtValue::F32(release), RtValue::F32(0.0), RtValue::F32(0.0)]
        .into_iter()
        .chain(roots)
        .chain(unheld)
        .chain(levels);
    let reserve = EventReserve {
        control: index_of(InstrumentControl::NoteOff, zones),
        places: NOTE_OFF_RESERVE,
    };
    let cells = Arc::new(ControlCells::queuing_events(
        values,
        NOTE_QUEUE,
        Some(reserve),
    ));
    let surface = DeclaredSurface::new(table.clone(), cells.clone());
    for (zone, spec) in specs.iter().enumerate() {
        surface.show_payload(&format!("asset.{zone}"), spec.asset.as_str().into());
    }
    let loops = ZoneLoops(specs.iter().map(|spec| spec.loop_spec).collect());
    let surface = surface
        .with_context(Arc::new(loader))
        .with_context(Arc::new(loops));
    Ok(BuiltControls {
        table,
        cells,
        surface,
        audio,
    })
}

/// `number`, rounded, as a MIDI note; `None` past MIDI's range.
pub(super) fn midi_note(number: f32) -> Option<u8> {
    let note = number.round();
    (0.0..=127.0).contains(&note).then_some(note as u8)
}

/// Loads `asset` through `loader` (the shared decoded-buffer cache) and
/// scales its sustain loop (authored in source frames) to the engine rate.
fn zone_audio(
    loader: &AssetLoader,
    asset: &str,
    loop_spec: Option<&LoopSpec>,
) -> Result<ZoneAudio, String> {
    let sample = loader.load(asset)?;
    let loop_region = loop_spec
        .map(|spec| scale_loop(spec, &sample))
        .transpose()?;
    Ok(ZoneAudio {
        sample,
        loop_region,
    })
}

fn scale_loop(spec: &LoopSpec, sample: &SampleData) -> Result<ScaledLoop, String> {
    let start = sample.scaled_frame(spec.start_frames)? as f64;
    let end = sample.scaled_frame(spec.end_frames)? as f64;
    let crossfade = sample.scaled_frame(spec.crossfade_frames)? as f64;
    if end <= start {
        return Err("loop is too short after resampling".to_string());
    }
    if end > sample.len() as f64 {
        return Err(format!(
            "loop end_frames is past the end of the sample ({} frames at the engine rate)",
            sample.len()
        ));
    }
    if crossfade > start {
        return Err(
            "loop crossfade_frames must not exceed start_frames (the crossfade reads the \
             audio just before the loop start)"
                .to_string(),
        );
    }
    if crossfade >= end - start {
        return Err("loop crossfade_frames must be shorter than the loop".to_string());
    }
    Ok(ScaledLoop {
        start,
        end,
        crossfade,
    })
}
