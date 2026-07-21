//! Thread-safe controls for the SampleInstrument module.
//!
//! All non-realtime work (asset resolution, decode, resample, loop-point
//! scaling) happens in control-thread calls here; the audio thread only
//! observes atomics and, when staged work exists, takes one short lock
//! gated behind a version counter (the pattern from `SampleKitControls`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::atomic::AtomicF32;
use crate::modules::sample_loading::{load_cached_sample, resolve_source, SampleData};
use crate::{ControlMeta, ControlSurface, ControlValue};

/// Upper bound on control-thread note events staged per block. Both the
/// pending queue and the audio thread's drain scratch are sized to this,
/// so draining never allocates.
pub(crate) const MAX_PENDING_NOTES: usize = 64;

/// One keymap zone as authored in config: a sample, the note it was
/// recorded at, the key range it covers, and an optional sustain loop.
pub struct ZoneSpec {
    pub root: u8,
    pub key_low: u8,
    pub key_high: u8,
    pub asset: String,
    pub gain: f32,
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

/// The buffer and loop a zone plays. Swapped wholesale when `asset.<i>`
/// re-points the zone, so a voice can never see a loop from one file
/// against the buffer of another.
pub(crate) struct ZoneAudio {
    pub sample: Arc<SampleData>,
    pub loop_region: Option<ScaledLoop>,
}

/// A control-thread note request, drained per block by the audio thread.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NoteEvent {
    pub note: u8,
    pub on: bool,
}

#[derive(Clone)]
pub struct SampleInstrumentControls {
    inner: Arc<InstrumentInner>,
}

struct InstrumentInner {
    sample_rate: u32,
    release: AtomicF32,
    zones: Vec<ZoneState>,
    /// Bumped after staging an asset swap in `pending_swaps`; the audio
    /// thread locks only when this counter moved.
    swaps_version: AtomicU64,
    pending_swaps: Mutex<Vec<Option<ZoneAudio>>>,
    /// Bumped after pushing a note event; same lock discipline.
    notes_version: AtomicU64,
    pending_notes: Mutex<Vec<NoteEvent>>,
}

struct ZoneState {
    root: u8,
    key_low: u8,
    key_high: u8,
    gain: AtomicF32,
    /// Authored loop points, kept in source frames so an asset swap can
    /// re-scale them against the new file's rate.
    loop_spec: Option<LoopSpec>,
    /// Authored asset ref, kept as the control value so saved documents
    /// stay portable instead of carrying this machine's cache path.
    asset: Mutex<String>,
}

impl SampleInstrumentControls {
    /// Builds controls from parsed zone specs, loading every referenced
    /// sample (decode + resample + loop scaling happen here, off the audio
    /// thread). Returns the controls and each zone's playable audio, in
    /// zone order.
    pub(crate) fn new(
        sample_rate: u32,
        release: f32,
        specs: Vec<ZoneSpec>,
    ) -> Result<(Self, Vec<ZoneAudio>), String> {
        let mut audio = Vec::with_capacity(specs.len());
        let mut zones = Vec::with_capacity(specs.len());
        for (index, spec) in specs.into_iter().enumerate() {
            let zone_audio = load_zone_audio(&spec.asset, spec.loop_spec.as_ref(), sample_rate)
                .map_err(|err| format!("zones[{}]: {}", index, err))?;
            audio.push(zone_audio);
            zones.push(ZoneState {
                root: spec.root,
                key_low: spec.key_low,
                key_high: spec.key_high,
                gain: AtomicF32::new(spec.gain.clamp(0.0, 2.0)),
                loop_spec: spec.loop_spec,
                asset: Mutex::new(spec.asset),
            });
        }

        let pending = (0..zones.len()).map(|_| None).collect();
        let controls = Self {
            inner: Arc::new(InstrumentInner {
                sample_rate,
                release: AtomicF32::new(release),
                zones,
                swaps_version: AtomicU64::new(0),
                pending_swaps: Mutex::new(pending),
                notes_version: AtomicU64::new(0),
                pending_notes: Mutex::new(Vec::with_capacity(MAX_PENDING_NOTES)),
            }),
        };
        Ok((controls, audio))
    }

    pub fn zone_count(&self) -> usize {
        self.inner.zones.len()
    }

    /// The authored root note and key range of zone `index`.
    pub fn zone_key(&self, index: usize) -> Option<(u8, u8, u8)> {
        self.inner
            .zones
            .get(index)
            .map(|zone| (zone.root, zone.key_low, zone.key_high))
    }

    pub fn gain(&self, index: usize) -> f32 {
        self.inner
            .zones
            .get(index)
            .map(|zone| zone.gain.load())
            .unwrap_or(0.0)
    }

    pub fn set_gain(&self, index: usize, gain: f32) -> Result<(), String> {
        let zone = self.zone(index)?;
        zone.gain.store(gain.clamp(0.0, 2.0));
        Ok(())
    }

    pub fn release(&self) -> f32 {
        self.inner.release.load()
    }

    pub fn set_release(&self, release: f32) -> Result<(), String> {
        if !release.is_finite() || release <= 0.0 {
            return Err("'release' must be a positive number of seconds".to_string());
        }
        self.inner.release.store(release.clamp(1e-3, 30.0));
        Ok(())
    }

    pub fn asset(&self, index: usize) -> Result<String, String> {
        Ok(self.zone(index)?.asset.lock().unwrap().clone())
    }

    /// Re-points zone `index` at a new asset: resolves, loads, and re-scales
    /// the zone's loop points against the new file here (off the audio
    /// thread), then stages the result for the audio thread to swap in at
    /// the next block boundary. Voices already sounding keep the old buffer.
    pub fn set_asset(&self, index: usize, asset: &str) -> Result<(), String> {
        let zone = self.zone(index)?;
        let audio = load_zone_audio(asset, zone.loop_spec.as_ref(), self.inner.sample_rate)?;
        *zone.asset.lock().unwrap() = asset.to_string();
        self.inner.pending_swaps.lock().unwrap()[index] = Some(audio);
        self.inner.swaps_version.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Requests a note start (full velocity) from the control thread.
    pub fn note_on(&self, value: &ControlValue) -> Result<(), String> {
        self.push_note(parse_note(value)?, true)
    }

    /// Requests a note release from the control thread.
    pub fn note_off(&self, value: &ControlValue) -> Result<(), String> {
        self.push_note(parse_note(value)?, false)
    }

    fn push_note(&self, note: u8, on: bool) -> Result<(), String> {
        let mut pending = self.inner.pending_notes.lock().unwrap();
        if pending.len() >= MAX_PENDING_NOTES {
            return Err("Too many pending note events".to_string());
        }
        pending.push(NoteEvent { note, on });
        drop(pending);
        self.inner.notes_version.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Audio thread: the current swap counter, checked once per block.
    pub(crate) fn swaps_version(&self) -> u64 {
        self.inner.swaps_version.load(Ordering::Acquire)
    }

    /// Audio thread: moves staged swaps into `into` (zone-indexed). Called
    /// only after `swaps_version` moved, so the lock is rarely taken.
    pub(crate) fn take_swaps(&self, into: &mut [Option<ZoneAudio>]) {
        let mut pending = self.inner.pending_swaps.lock().unwrap();
        for (target, staged) in into.iter_mut().zip(pending.iter_mut()) {
            if let Some(audio) = staged.take() {
                *target = Some(audio);
            }
        }
    }

    /// Audio thread: the current note-event counter, checked once per block.
    pub(crate) fn notes_version(&self) -> u64 {
        self.inner.notes_version.load(Ordering::Acquire)
    }

    /// Audio thread: drains pending note events into `into`, which must be
    /// pre-allocated with capacity `MAX_PENDING_NOTES` (never exceeded, so
    /// the push never allocates).
    pub(crate) fn take_note_events(&self, into: &mut Vec<NoteEvent>) {
        let mut pending = self.inner.pending_notes.lock().unwrap();
        for event in pending.drain(..) {
            if into.len() < into.capacity() {
                into.push(event);
            }
        }
    }

    fn zone(&self, index: usize) -> Result<&ZoneState, String> {
        self.inner
            .zones
            .get(index)
            .ok_or_else(|| format!("No zone {}", index))
    }
}

/// Parses a note control value: a MIDI note number, or a numeric string.
fn parse_note(value: &ControlValue) -> Result<u8, String> {
    let number = match value {
        ControlValue::Number(number) => *number,
        ControlValue::String(text) => text
            .trim()
            .parse::<f32>()
            .map_err(|_| format!("Invalid note '{}': expected a MIDI note number", text))?,
        ControlValue::Bool(_) => {
            return Err("Expected a MIDI note number, not a boolean".to_string())
        }
    };
    if !number.is_finite() {
        return Err(format!("Invalid note {}", number));
    }
    let note = number.round();
    if !(0.0..=127.0).contains(&note) {
        return Err(format!("Note {} is outside MIDI range 0..=127", number));
    }
    Ok(note as u8)
}

/// Resolves an authored ref through the shared decoded-buffer cache and
/// scales its sustain loop (authored in source frames) to the engine rate.
fn load_zone_audio(
    asset: &str,
    loop_spec: Option<&LoopSpec>,
    sample_rate: u32,
) -> Result<ZoneAudio, String> {
    let resolved = resolve_source(asset)?;
    let sample = load_cached_sample(&resolved, sample_rate)?;
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

impl ControlSurface for SampleInstrumentControls {
    fn controls(&self) -> Vec<ControlMeta> {
        let mut metas = vec![
            ControlMeta::number(
                "release",
                "Release fade time in seconds after a note's gate falls",
            )
            .with_range(0.001, 30.0)
            .with_default(self.release()),
            ControlMeta::number("note_on", "Start a note by MIDI note number (full velocity)"),
            ControlMeta::number("note_off", "Release a held note by MIDI note number"),
        ];
        for (index, zone) in self.inner.zones.iter().enumerate() {
            metas.push(
                ControlMeta::number(
                    format!("root.{}", index),
                    format!(
                        "Zone root note, covering keys {}..={} (fixed at build time)",
                        zone.key_low, zone.key_high
                    ),
                )
                .with_default(zone.root as f32),
            );
            metas.push(
                ControlMeta::string(
                    format!("asset.{}", index),
                    "Zone sample: path, https URL, or package ref like \
                     'fugue.keys.grand@1.0.0:c4.wav' (WAV or FLAC)",
                )
                .with_default(zone.asset.lock().unwrap().clone()),
            );
            metas.push(
                ControlMeta::number(format!("gain.{}", index), "Zone level (1.0 = unity)")
                    .with_range(0.0, 2.0)
                    .with_default(zone.gain.load()),
            );
        }
        metas
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "release" => return Ok(self.release().into()),
            "note_on" | "note_off" => return Ok(0.0.into()),
            _ => {}
        }
        if let Some(index) = indexed_key(key, "root.") {
            return Ok((self.zone(index)?.root as f32).into());
        }
        if let Some(index) = indexed_key(key, "asset.") {
            return Ok(self.asset(index)?.into());
        }
        if let Some(index) = indexed_key(key, "gain.") {
            self.zone(index)?;
            return Ok(self.gain(index).into());
        }
        Err(format!("Unknown control: {}", key))
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "release" => return self.set_release(value.as_number()?),
            "note_on" => return self.note_on(&value),
            "note_off" => return self.note_off(&value),
            _ => {}
        }
        if let Some(index) = indexed_key(key, "root.") {
            self.zone(index)?;
            return Err(
                "Zone roots and ranges are fixed at build time; edit the invention config"
                    .to_string(),
            );
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

/// Parses hierarchical control keys like `gain.3` into the zone index.
fn indexed_key(key: &str, prefix: &str) -> Option<usize> {
    key.strip_prefix(prefix)?.parse().ok()
}
