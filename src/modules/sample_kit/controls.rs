//! Thread-safe controls for the SampleKit module.
//!
//! All non-realtime work (asset resolution, decode, resample) happens in
//! control-thread calls here; the audio thread only observes atomics and,
//! when a staged sample swap exists, takes one short lock gated behind
//! `swaps_version` (the counter pattern from `AgentControls` /
//! `cell_sequencer`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::atomic::AtomicF32;
use crate::modules::sample_loading::{load_cached_sample, resolve_source, SampleData};
use crate::{ControlMeta, ControlSurface, ControlValue};

/// The key a slot answers to: an integer (a trigger's value / MIDI note
/// number) or a name (triggered from control and script threads).
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
    pub gain: f32,
}

#[derive(Clone)]
pub struct SampleKitControls {
    inner: Arc<KitInner>,
}

struct KitInner {
    sample_rate: u32,
    slots: Vec<SlotState>,
    /// Bumped after staging a sample swap in `pending`; the audio thread
    /// locks `pending` only when this counter moved.
    swaps_version: AtomicU64,
    pending: Mutex<Vec<Option<Arc<SampleData>>>>,
}

struct SlotState {
    key: SlotKey,
    gain: AtomicF32,
    /// Trigger requests from control/script threads: incremented here,
    /// observed per block by the audio thread.
    trigger_count: AtomicU64,
    /// Authored asset ref, kept as the control value so saved documents
    /// stay portable instead of carrying this machine's cache path.
    asset: Mutex<String>,
}

impl SampleKitControls {
    /// Builds controls from parsed slot specs, loading every referenced
    /// sample (decode + resample happen here, off the audio thread). Returns
    /// the controls and the preloaded buffer for each slot, in slot order.
    pub(crate) fn new(
        sample_rate: u32,
        specs: Vec<SlotSpec>,
    ) -> Result<(Self, Vec<Arc<SampleData>>), String> {
        let mut samples = Vec::with_capacity(specs.len());
        let mut slots = Vec::with_capacity(specs.len());
        for spec in specs {
            let sample = load_slot_sample(&spec.asset, sample_rate)
                .map_err(|err| format!("slot '{}': {}", spec.key, err))?;
            samples.push(sample);
            slots.push(SlotState {
                key: spec.key,
                gain: AtomicF32::new(spec.gain.clamp(0.0, 2.0)),
                trigger_count: AtomicU64::new(0),
                asset: Mutex::new(spec.asset),
            });
        }

        let pending = (0..slots.len()).map(|_| None).collect();
        let controls = Self {
            inner: Arc::new(KitInner {
                sample_rate,
                slots,
                swaps_version: AtomicU64::new(0),
                pending: Mutex::new(pending),
            }),
        };
        Ok((controls, samples))
    }

    pub fn slot_count(&self) -> usize {
        self.inner.slots.len()
    }

    /// The authored key of slot `index`.
    pub fn key(&self, index: usize) -> Option<SlotKey> {
        self.inner.slots.get(index).map(|slot| slot.key.clone())
    }

    pub fn gain(&self, index: usize) -> f32 {
        self.inner
            .slots
            .get(index)
            .map(|slot| slot.gain.load())
            .unwrap_or(0.0)
    }

    pub fn set_gain(&self, index: usize, gain: f32) -> Result<(), String> {
        let slot = self.slot(index)?;
        slot.gain.store(gain.clamp(0.0, 2.0));
        Ok(())
    }

    pub fn asset(&self, index: usize) -> Result<String, String> {
        Ok(self.slot(index)?.asset.lock().unwrap().clone())
    }

    /// Re-points slot `index` at a new asset: resolves and loads the sample
    /// here (off the audio thread), then stages it for the audio thread to
    /// swap in at the next block boundary.
    pub fn set_asset(&self, index: usize, asset: &str) -> Result<(), String> {
        let slot = self.slot(index)?;
        let sample = load_slot_sample(asset, self.inner.sample_rate)?;
        *slot.asset.lock().unwrap() = asset.to_string();
        self.inner.pending.lock().unwrap()[index] = Some(sample);
        self.inner.swaps_version.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Requests a trigger of the slot matching `value`: a number (or numeric
    /// string) matches an integer key, any other string matches a named key.
    pub fn trigger(&self, value: &ControlValue) -> Result<(), String> {
        let index = match value {
            ControlValue::Number(number) => {
                if !number.is_finite() {
                    return Err(format!("Invalid slot key {}", number));
                }
                self.find_numeric(number.round() as i32)?
            }
            ControlValue::String(text) => match text.trim().parse::<i32>() {
                Ok(key) => self.find_numeric(key)?,
                Err(_) => self.find_named(text.trim())?,
            },
            ControlValue::Bool(_) => {
                return Err("Expected a slot key or name, not a boolean".to_string())
            }
        };
        self.inner.slots[index]
            .trigger_count
            .fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Audio thread: the current swap counter, checked once per block.
    pub(crate) fn swaps_version(&self) -> u64 {
        self.inner.swaps_version.load(Ordering::Acquire)
    }

    /// Audio thread: moves staged swaps into `into` (slot-indexed). Called
    /// only after `swaps_version` moved, so the lock is rarely taken.
    pub(crate) fn take_swaps(&self, into: &mut [Option<Arc<SampleData>>]) {
        let mut pending = self.inner.pending.lock().unwrap();
        for (target, staged) in into.iter_mut().zip(pending.iter_mut()) {
            if let Some(sample) = staged.take() {
                *target = Some(sample);
            }
        }
    }

    /// Audio thread: the trigger-request counter for slot `index`.
    pub(crate) fn trigger_count(&self, index: usize) -> u64 {
        self.inner.slots[index]
            .trigger_count
            .load(Ordering::Acquire)
    }

    fn slot(&self, index: usize) -> Result<&SlotState, String> {
        self.inner
            .slots
            .get(index)
            .ok_or_else(|| format!("No sample slot {}", index))
    }

    fn find_numeric(&self, key: i32) -> Result<usize, String> {
        self.inner
            .slots
            .iter()
            .position(|slot| slot.key == SlotKey::Number(key))
            .ok_or_else(|| format!("No sample slot with key {}", key))
    }

    fn find_named(&self, name: &str) -> Result<usize, String> {
        self.inner
            .slots
            .iter()
            .position(|slot| matches!(&slot.key, SlotKey::Name(slot_name) if slot_name == name))
            .ok_or_else(|| format!("No sample slot named '{}'", name))
    }
}

/// Resolves an authored ref (package ref, path, or https URL) and loads it
/// through the shared decoded-buffer cache.
fn load_slot_sample(asset: &str, sample_rate: u32) -> Result<Arc<SampleData>, String> {
    let resolved = resolve_source(asset)?;
    load_cached_sample(&resolved, sample_rate)
}

impl ControlSurface for SampleKitControls {
    fn controls(&self) -> Vec<ControlMeta> {
        let mut metas = vec![ControlMeta::string(
            "trigger",
            "Trigger a slot by key or name (e.g. '36' or 'kick')",
        )];
        for (index, slot) in self.inner.slots.iter().enumerate() {
            metas.push(
                ControlMeta::string(format!("key.{}", index), "Slot key (fixed at build time)")
                    .with_default(slot.key.to_string()),
            );
            metas.push(
                ControlMeta::string(
                    format!("asset.{}", index),
                    "Slot sample: path, https URL, or package ref like \
                     'fugue.drums.808@1.2.0:kick.wav' (WAV or FLAC)",
                )
                .with_default(slot.asset.lock().unwrap().clone()),
            );
            metas.push(
                ControlMeta::number(format!("gain.{}", index), "Slot level (1.0 = unity)")
                    .with_range(0.0, 2.0)
                    .with_default(slot.gain.load()),
            );
        }
        metas
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        if key == "trigger" {
            return Ok(String::new().into());
        }
        if let Some(index) = indexed_key(key, "key.") {
            return Ok(self.slot(index)?.key.to_string().into());
        }
        if let Some(index) = indexed_key(key, "asset.") {
            return Ok(self.asset(index)?.into());
        }
        if let Some(index) = indexed_key(key, "gain.") {
            self.slot(index)?;
            return Ok(self.gain(index).into());
        }
        Err(format!("Unknown control: {}", key))
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        if key == "trigger" {
            return self.trigger(&value);
        }
        if let Some(index) = indexed_key(key, "key.") {
            self.slot(index)?;
            return Err("Slot keys are fixed at build time; edit the invention config".to_string());
        }
        if let Some(index) = indexed_key(key, "asset.") {
            return self.set_asset(index, value.as_string()?);
        }
        if let Some(index) = indexed_key(key, "gain.") {
            return self.set_gain(index, value.as_number()?);
        }
        Err(format!("Unknown control: {}", key))
    }
}

/// Parses hierarchical control keys like `gain.3` into the slot index.
fn indexed_key(key: &str, prefix: &str) -> Option<usize> {
    key.strip_prefix(prefix)?.parse().ok()
}
