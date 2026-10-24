//! The SampleKit's controls, all declared, in a table built for each kit
//! since its slot count is fixed at build: `play`, and per slot `asset.N`,
//! `level.N` and `key.N`. A slot past the count is no control at all.
//!
//! - `play` is an event naming a slot by key or name, read on a control
//!   thread into the slot's index; a key or name no slot has is refused
//!   with the reason.
//! - `asset.N` is a sample loaded on a control thread at the engine rate
//!   (`SAMPLE_ASSET`), adopted by the kit at a block boundary; the sample it
//!   replaces is retired off the audio thread.
//! - `level.N` is a number the kit clamps to 0..2.
//! - `key.N` reads back the slot's key; it is fixed at build.

use std::sync::Arc;

use crate::control_request::{
    ControlCells, ControlContext, ControlDecl, ControlIndex, ControlTable, DeclKind, PayloadCodec,
    Prepared, RtValue, ValueReader,
};
use crate::invention::declared::DeclaredSurface;
use crate::modules::sample_loading::{AssetLoader, SampleData, SAMPLE_ASSET};
use crate::payload::Shared;
use crate::ControlValue;

/// The key a slot answers to: an integer (a `play` value / MIDI note
/// number) or a name (played from control and script threads).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotKey {
    Number(i32),
    Name(String),
}

impl std::fmt::Display for SlotKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SlotKey::Number(key) => write!(f, "{}", key),
            SlotKey::Name(name) => write!(f, "{}", name),
        }
    }
}

/// One slot as authored in config: its key, asset reference, and level.
pub struct SlotSpec {
    pub key: SlotKey,
    pub asset: String,
    pub level: f32,
}

/// What a control index of a kit's table names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KitControl {
    Play,
    Asset(usize),
    Level(usize),
    Key(usize),
}

/// The index of `control` in the table of a kit of `slots` slots: `play`,
/// then each slot's `asset`, `level` and `key` in turn.
pub(super) fn index_of(control: KitControl, slots: usize) -> ControlIndex {
    let index = match control {
        KitControl::Play => 0,
        KitControl::Asset(slot) => 1 + slot,
        KitControl::Level(slot) => 1 + slots + slot,
        KitControl::Key(slot) => 1 + 2 * slots + slot,
    };
    ControlIndex(index as u16)
}

/// What `index` names in the table of a kit of `slots` slots.
pub(super) fn control_at(index: ControlIndex, slots: usize) -> Option<KitControl> {
    let index = usize::from(index.0);
    if index == 0 {
        return Some(KitControl::Play);
    }
    let (group, slot) = ((index - 1) / slots.max(1), (index - 1) % slots.max(1));
    match group {
        _ if slot >= slots => None,
        0 => Some(KitControl::Asset(slot)),
        1 => Some(KitControl::Level(slot)),
        2 => Some(KitControl::Key(slot)),
        _ => None,
    }
}

/// The most slots a kit takes: one control index per slot control.
pub(super) const MAX_SLOTS: usize = (u16::MAX as usize - 1) / 3;

/// The slot keys a kit was built with, attached to its surface for `play`.
struct SlotKeys(Vec<SlotKey>);

/// Reads a `play` value as the slot it names: a number (or numeric string)
/// names an integer key, any other string a named one.
const SLOT: ValueReader = ValueReader { read: read_slot };

fn read_slot(context: &ControlContext<'_>, value: &ControlValue) -> Result<RtValue, String> {
    let SlotKeys(keys) = context.module::<SlotKeys>()?;
    let find = |key: SlotKey| keys.iter().position(|slot| *slot == key);
    let slot = match value {
        ControlValue::Number(number) => {
            if !number.is_finite() {
                return Err(format!("Invalid slot key {}", number));
            }
            let key = number.round() as i32;
            find(SlotKey::Number(key)).ok_or(format!("No sample slot with key {}", key))?
        }
        ControlValue::String(text) => match text.trim().parse::<i32>() {
            Ok(key) => {
                find(SlotKey::Number(key)).ok_or(format!("No sample slot with key {key}"))?
            }
            Err(_) => {
                let name = text.trim();
                (find(SlotKey::Name(name.to_string())))
                    .ok_or_else(|| format!("No sample slot named '{}'", name))?
            }
        },
        ControlValue::Bool(_) => {
            return Err("Expected a slot key or name, not a boolean".to_string())
        }
    };
    Ok(RtValue::I32(slot as i32))
}

/// `key.N` reads back its slot's key, and refuses every write.
const FIXED_KEY: PayloadCodec = PayloadCodec {
    prepare: refuse_key,
};

fn refuse_key(_: &ControlContext<'_>, _: &ControlValue) -> Prepared {
    Err("Slot keys are fixed at build time; edit the invention config".to_string())
}

/// The table of a kit of `slots` slots, in [`index_of`] order.
fn table(slots: usize) -> ControlTable {
    let count = slots as u16;
    let decls = vec![
        ControlDecl::new(
            "play",
            DeclKind::Integer {
                min: 0,
                max: slots.saturating_sub(1) as i32,
            },
            RtValue::I32(0),
            "Play a slot by key or name (e.g. '36' or 'kick')",
        )
        .event()
        .read_by(SLOT),
        ControlDecl::payload(
            "asset",
            SAMPLE_ASSET,
            "Slot sample: path, https URL, or package ref like \
             'fugue.drums.808@1.2.0:kick.wav' (WAV or FLAC)",
        )
        .loads_on_write()
        .indexed(count),
        ControlDecl::new(
            "level",
            DeclKind::Number { min: 0.0, max: 2.0 },
            RtValue::F32(1.0),
            "Slot level (1.0 = unity)",
        )
        .clamped(0.0, 2.0)
        .indexed(count),
        ControlDecl::payload("key", FIXED_KEY, "Slot key (fixed at build time)").indexed(count),
    ];
    ControlTable::built(decls).expect("a kit's table is valid for every slot count it takes")
}

/// A kit's controls as built: its table and cells, the surface clients
/// write, and each slot's sample, loaded at the engine rate.
pub(super) struct BuiltControls {
    pub(super) table: ControlTable,
    pub(super) cells: Arc<ControlCells>,
    pub(super) surface: DeclaredSurface,
    pub(super) samples: Vec<Shared<SampleData>>,
}

/// Builds a kit's controls from its slot specs, loading every sample here
/// (decode and resample happen on this thread, under its build lock).
pub(super) fn build(sample_rate: u32, specs: Vec<SlotSpec>) -> Result<BuiltControls, String> {
    let slots = specs.len();
    if slots > MAX_SLOTS {
        return Err(format!(
            "a kit takes at most {MAX_SLOTS} samples, not {slots}"
        ));
    }
    let loader = AssetLoader::current(sample_rate);
    let mut samples = Vec::with_capacity(slots);
    for spec in &specs {
        let sample =
            (loader.load(&spec.asset)).map_err(|err| format!("slot '{}': {}", spec.key, err))?;
        samples.push(Shared::from_arc(sample));
    }

    let table = table(slots);
    let levels = specs
        .iter()
        .map(|spec| RtValue::F32(spec.level.clamp(0.0, 2.0)));
    let unheld = || std::iter::repeat_n(RtValue::Bool(false), slots);
    let cells = std::iter::once(RtValue::I32(0))
        .chain(unheld())
        .chain(levels)
        .chain(unheld());
    let cells = Arc::new(ControlCells::new(cells));
    let surface = DeclaredSurface::new(table.clone(), cells.clone());
    for (slot, spec) in specs.iter().enumerate() {
        surface.show_payload(&format!("asset.{slot}"), spec.asset.as_str().into());
        surface.show_payload(&format!("key.{slot}"), spec.key.to_string().into());
    }
    let keys = SlotKeys(specs.into_iter().map(|spec| spec.key).collect());
    let surface = surface
        .with_context(Arc::new(loader))
        .with_context(Arc::new(keys));
    Ok(BuiltControls {
        table,
        cells,
        surface,
        samples,
    })
}
