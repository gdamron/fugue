//! Keymapped pitched-instrument sampler: the gate-shaped counterpart to the
//! trigger-shaped `sample_kit`.
//!
//! Config maps samples to root notes with key ranges (sparse multisampling —
//! sample every few semitones and let neighbouring keys pitch-shift). An
//! incoming note resolves to its containing/nearest zone, derives a pitch
//! ratio from the zone's root, and plays on a pre-allocated polyphonic voice
//! pool keyed by note — so two sounding notes that resolve to the same zone
//! never choke each other.
//!
//! ```json
//! {
//!   "type": "sample_instrument",
//!   "config": {
//!     "voice_count": 8,
//!     "release": 0.25,
//!     "zones": [
//!       { "root_note": 48, "key_range": [36, 53], "asset": "fugue.keys.grand@1.0.0:c3.wav" },
//!       { "root_note": 60, "key_range": [54, 65], "asset": "fugue.keys.grand@1.0.0:c4.wav",
//!         "loop": { "start_frames": 22050, "end_frames": 66150, "crossfade_frames": 2205 } },
//!       { "root_note": 72, "key_range": [66, 84], "asset": "fugue.keys.grand@1.0.0:c5.wav", "level": 0.9 }
//!     ]
//!   }
//! }
//! ```
//!
//! # Gate semantics
//!
//! A note starts on a `gate` rising edge — the `frequency` and `velocity`
//! inputs are latched at that instant — and releases on the falling edge.
//! The three inputs mirror a `divisi` voice trio (`frequency.N` / `gate.N` /
//! `velocity.N`), so divisi can fan a line across several instances, while a
//! single instance still absorbs overlapping release tails on its own pool.
//!
//! # Sustain looping
//!
//! A zone may declare loop points (in the source file's frames) so a short
//! sample holds a long note: while the gate is high the read head wraps the
//! loop (with an optional crossfade for un-authored loop points); on gate
//! fall playback exits the loop into the rest of the sample, fading over
//! the `release` time.
//!
//! # Pitch shift
//!
//! Resampling-style (coupled pitch/speed) via the shared cubic kernel in
//! `sample_loading` — the fractional read head advances by
//! `frequency / root_frequency` per frame. The elastic (time-preserving)
//! reader can replace this once uniform note durations across the keymap
//! are needed.
//!
//! # Controls
//!
//! All declared (see [`controls`]): `release`, the `note_on` / `note_off`
//! events, and per zone `root_note.N` (fixed), `asset.N` and `level.N`. A
//! note written by a control thread is a request, applied at a block
//! boundary in the order sent; a scheduler's notes are queued, every one
//! kept, so a chord at one step plays whole (see [`controls`]).
//!
//! # Inputs
//! - `frequency`: Note pitch in Hz, latched at note-on
//! - `gate`: Note gate; rising edge starts, falling edge releases
//! - `velocity`: Note level, latched at note-on (1.0 when unconnected)
//!
//! # Outputs
//! - `audio_left`, `audio_right`: Stereo mix of the voice pool

use std::sync::Arc;

use crate::control_request::{
    local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable, Refusal,
    RtValue,
};
use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::module_config::{finite_f32, whole_number, whole_number_in, ConfigKey, ConfigReader};
use crate::modules::sample_loading::check_entry_fields;
use crate::music::Note;
use crate::payload::{Payload, Retired, Shared};
use crate::{ControlMeta, Module};

pub use self::controls::{LoopSpec, ZoneSpec};
pub use self::voice::MAX_VOICES;

use self::controls::RELEASE_RANGE;
use self::controls::{control_at, index_of, midi_note, InstrumentControl, ZoneAudio};
use self::voice::{note_from_freq, resolve_zone, Voice, ZoneRuntime, DECLICK_SECONDS};

mod controls;
mod inputs;
mod outputs;
mod voice;

const DEFAULT_VOICE_COUNT: usize = 8;
const DEFAULT_RELEASE: f32 = 0.1;

pub struct SampleInstrumentFactory;

const TYPE_ID: &str = "sample_instrument";
const VOICE_COUNT: ConfigKey = ConfigKey::integer("voice_count", 1, MAX_VOICES as i128);
const RELEASE: ConfigKey = ConfigKey::float("release");
/// The fields a `zones` entry takes.
const ZONE_FIELDS: &[&str] = &["root_note", "key_range", "asset", "level", "loop"];

impl ModuleFactory for SampleInstrumentFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        const { &[VOICE_COUNT, RELEASE, ConfigKey::json("zones")] }
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let (specs, voices, release) = parse_config(config)?;
        let built = controls::build(sample_rate, release, &specs)?;
        // A recorded `level.N` write is later than the zone's authored
        // level, so it wins; a `root_note.N` is refused, as a live write is.
        crate::factory::apply_control_keys(&built.surface, config, |key| {
            key.starts_with("level.") || key.starts_with("root_note.")
        })?;
        let instrument = SampleInstrument::new(
            built.table,
            built.cells,
            &specs,
            built.audio,
            voices,
            sample_rate,
        );

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(instrument)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(built.surface)),
            sink: None,
        })
    }
}

/// Parses the `zones` list plus pool-wide settings. A missing or null
/// config builds an empty instrument (module type discovery constructs
/// every type with a null config).
fn parse_config(config: &serde_json::Value) -> Result<(Vec<ZoneSpec>, usize, f32), String> {
    let reader = ConfigReader::new(TYPE_ID, config);
    let voices = reader
        .int::<usize>(&VOICE_COUNT)
        .map_err(|error| error.to_string())?
        .unwrap_or(DEFAULT_VOICE_COUNT);

    let release = match reader.float(&RELEASE).map_err(|error| error.to_string())? {
        None => DEFAULT_RELEASE,
        Some(release) if release > 0.0 => release.clamp(RELEASE_RANGE.0, RELEASE_RANGE.1),
        Some(_) => return Err("'release' must be a positive number of seconds".to_string()),
    };

    let Some(zones) = config.get("zones") else {
        return Ok((Vec::new(), voices, release));
    };
    let Some(entries) = zones.as_array() else {
        return Err("'zones' must be an array of zone objects".to_string());
    };

    let mut specs: Vec<ZoneSpec> = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let zone = entry
            .as_object()
            .ok_or_else(|| format!("zones[{}] must be an object", index))?;
        specs.push(parse_zone(zone, index, &reader)?);
    }
    Ok((specs, voices, release))
}

fn parse_zone(
    zone: &serde_json::Map<String, serde_json::Value>,
    index: usize,
    reader: &ConfigReader,
) -> Result<ZoneSpec, String> {
    check_entry_fields(zone, &format!("zones[{index}]"), ZONE_FIELDS)?;
    let midi_note = |value, path: String| {
        whole_number_in::<u8>(value, 0, 127).map_err(|r| reader.refuse(&path, r).to_string())
    };
    let root_note = zone.get("root_note").ok_or_else(|| {
        format!(
            "zones[{}]: 'root_note' must be a MIDI note number 0..=127",
            index
        )
    })?;
    let root_note = midi_note(root_note, format!("zones[{index}].root_note"))?;

    let (key_low, key_high) = match zone.get("key_range") {
        // A zone without a range covers only its root note; other notes reach
        // it through nearest-zone resolution.
        None => (root_note, root_note),
        Some(value) => {
            let range = value
                .as_array()
                .filter(|range| range.len() == 2)
                .ok_or_else(|| format!("zones[{}]: 'key_range' must be [low, high]", index))?;
            let low = midi_note(&range[0], format!("zones[{index}].key_range[0]"))?;
            let high = midi_note(&range[1], format!("zones[{index}].key_range[1]"))?;
            if low > high {
                return Err(format!("zones[{}]: 'key_range' low exceeds high", index));
            }
            (low, high)
        }
    };

    let asset = zone
        .get("asset")
        .ok_or_else(|| format!("zones[{}] is missing 'asset'", index))?;
    let asset: crate::pkg::AudioAssetRef = serde_json::from_value(asset.clone())
        .map_err(|err| format!("zones[{}]: invalid asset reference: {}", index, err))?;
    let asset = match asset {
        crate::pkg::AudioAssetRef::Text(text) => text,
        crate::pkg::AudioAssetRef::Local { path } => path,
    };

    let level = match zone.get("level") {
        Some(value) => finite_f32(value).map_err(|r| {
            reader
                .refuse(&format!("zones[{index}].level"), r)
                .to_string()
        })?,
        None => 1.0,
    };

    let loop_spec = zone
        .get("loop")
        .map(|value| parse_loop(value, index, reader))
        .transpose()?;

    Ok(ZoneSpec {
        root_note,
        key_low,
        key_high,
        asset,
        level,
        loop_spec,
    })
}

fn parse_loop(
    value: &serde_json::Value,
    index: usize,
    reader: &ConfigReader,
) -> Result<LoopSpec, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("zones[{}]: 'loop' must be an object", index))?;
    let frames = |key: &str| {
        object.get(key).map(|value| {
            whole_number::<u64>(value).map_err(|r| {
                reader
                    .refuse(&format!("zones[{index}].loop.{key}"), r)
                    .to_string()
            })
        })
    };
    let start_frames = frames("start_frames")
        .ok_or_else(|| format!("zones[{}]: loop is missing 'start_frames'", index))??;
    let end_frames = frames("end_frames")
        .ok_or_else(|| format!("zones[{}]: loop is missing 'end_frames'", index))??;
    let crossfade_frames = frames("crossfade_frames").transpose()?.unwrap_or(0);
    if end_frames <= start_frames {
        return Err(format!(
            "zones[{}]: loop end_frames must be greater than start_frames",
            index
        ));
    }
    Ok(LoopSpec {
        start_frames,
        end_frames,
        crossfade_frames,
    })
}

pub struct SampleInstrument {
    table: ControlTable,
    cells: Arc<ControlCells>,
    inputs: inputs::SampleInstrumentInputs,
    outputs: outputs::SampleInstrumentOutputs,
    zones: Vec<ZoneRuntime>,
    voices: Vec<Voice>,
    /// Zone levels as the mix plays them: taken from `level_targets` as a
    /// block begins, after its notes, so a note stealing a voice fades the
    /// old note out at the level it was sounding at.
    levels: Vec<f32>,
    /// Zone levels as last written.
    level_targets: Vec<f32>,
    /// Release fade, in seconds.
    release: f32,
    /// Whether a note off written since the last block waits for its
    /// release fade (see [`Module::finish_events`]).
    releasing: bool,
    /// Allocation ordinal handed to the next note-on (steal-oldest order).
    next_started: u64,
    sample_rate: f32,
    /// Per-frame decay of a stolen voice's declick ramp, fixed at build.
    declick_step: f32,
    last_gate: f32,
}

impl SampleInstrument {
    fn new(
        table: ControlTable,
        cells: Arc<ControlCells>,
        specs: &[ZoneSpec],
        audio: Vec<Shared<ZoneAudio>>,
        voices: usize,
        sample_rate: u32,
    ) -> Self {
        let zone_count = audio.len();
        let zones = (audio.into_iter().zip(specs))
            .map(|(audio, spec)| ZoneRuntime {
                audio,
                key_low: spec.key_low,
                key_high: spec.key_high,
                root_note: spec.root_note,
                root_freq: Note::new(spec.root_note).frequency(),
            })
            .collect();

        // From the cells, so a level recorded in the config holds from the
        // first frame, including a steal fade from a first-block note; a
        // write made before the build is applied as the instrument binds.
        let number = |control, default| match cells.load(index_of(control, zone_count)) {
            Some(RtValue::F32(value)) => value,
            _ => default,
        };
        let levels: Vec<f32> = (0..zone_count)
            .map(|zone| number(InstrumentControl::Level(zone), 1.0))
            .collect();
        let release = number(InstrumentControl::Release, DEFAULT_RELEASE);
        Self {
            table,
            cells,
            inputs: inputs::SampleInstrumentInputs::new(),
            outputs: outputs::SampleInstrumentOutputs::new(),
            zones,
            voices: (0..voices.clamp(1, MAX_VOICES))
                .map(|_| Voice::new())
                .collect(),
            level_targets: levels.clone(),
            levels,
            release,
            releasing: false,
            next_started: 0,
            sample_rate: sample_rate as f32,
            declick_step: 1.0 / (DECLICK_SECONDS * sample_rate as f32).max(1.0),
            last_gate: 0.0,
        }
    }

    fn control(&self, index: ControlIndex) -> Option<InstrumentControl> {
        control_at(index, self.zones.len())
    }

    /// The release fade's per-frame slope.
    fn release_step(&self) -> f32 {
        1.0 / (self.release.max(RELEASE_RANGE.0) * self.sample_rate)
    }

    fn note_on(&mut self, freq: f32, velocity: f32) {
        if !freq.is_finite() || freq <= 0.0 {
            return;
        }
        let note = note_from_freq(freq);
        let Some(zone_index) = resolve_zone(&self.zones, note) else {
            return;
        };
        let ratio = f64::from(freq) / f64::from(self.zones[zone_index].root_freq);

        let slot = self.claim_voice(note);
        // Taking over a sounding voice would cut its waveform mid-cycle;
        // hand the old note to the crossfade slot so it rings out instead.
        if self.voices[slot].active {
            let voice = &self.voices[slot];
            let amp = voice.velocity * self.levels[voice.zone] * voice.release_gain;
            self.voices[slot].begin_steal_fade(amp, self.declick_step);
        }
        self.voices[slot].start(
            note,
            zone_index,
            &self.zones[zone_index],
            ratio,
            velocity,
            self.next_started,
        );
        self.next_started = self.next_started.wrapping_add(1);
    }

    /// Picks the voice for a note-on. Keyed by note: a note already
    /// sounding retriggers its own voice, so distinct notes sharing a
    /// sparse zone never choke each other. Otherwise an idle voice, then
    /// steal-oldest (preferring voices already releasing).
    fn claim_voice(&self, note: u8) -> usize {
        let mut idle = None;
        let mut oldest_releasing: Option<(usize, u64)> = None;
        let mut oldest: Option<(usize, u64)> = None;
        for (index, voice) in self.voices.iter().enumerate() {
            if !voice.active {
                idle.get_or_insert(index);
                continue;
            }
            if voice.note == note {
                return index;
            }
            if !voice.held
                && oldest_releasing
                    .map(|(_, s)| voice.started < s)
                    .unwrap_or(true)
            {
                oldest_releasing = Some((index, voice.started));
            }
            if oldest.map(|(_, s)| voice.started < s).unwrap_or(true) {
                oldest = Some((index, voice.started));
            }
        }
        idle.or(oldest_releasing.map(|(index, _)| index))
            .or(oldest.map(|(index, _)| index))
            .unwrap_or(0)
    }

    /// Releases the held voice keyed by `note`. With `fallback_newest`
    /// (the gate-input path, where a falling edge always means "the note
    /// that just ended") an unmatched note releases the most recently
    /// started held voice instead; the explicit `note_off` control stays
    /// strictly keyed, so releasing a note the pool already stole is a
    /// no-op rather than cutting an unrelated held note.
    fn note_off(&mut self, note: u8, release_step: f32, fallback_newest: bool) {
        let mut matched = None;
        let mut newest_held: Option<(usize, u64)> = None;
        for (index, voice) in self.voices.iter().enumerate() {
            if !voice.active || !voice.held {
                continue;
            }
            if voice.note == note {
                matched = Some(index);
                break;
            }
            if newest_held.map(|(_, s)| voice.started > s).unwrap_or(true) {
                newest_held = Some((index, voice.started));
            }
        }
        let fallback = newest_held
            .filter(|_| fallback_newest)
            .map(|(index, _)| index);
        if let Some(index) = matched.or(fallback) {
            self.voices[index].release(release_step);
        }
    }
}

impl Module for SampleInstrument {
    fn name(&self) -> &str {
        "SampleInstrument"
    }

    fn process(&mut self, frames: usize) -> bool {
        let release_step = self.release_step();
        // A note off written for this sample fades over the release in
        // force as it sounds: one written beside it applies, whichever
        // came first, and one written later never reaches back.
        self.finish_events();

        // Levels are control-rate: taken once per block, after the notes
        // the block's writes played.
        self.levels.copy_from_slice(&self.level_targets);

        for i in 0..frames {
            let gate = self.inputs.gate(i);
            if gate > 0.5 && self.last_gate <= 0.5 {
                self.note_on(self.inputs.frequency(i), self.inputs.velocity(i));
            } else if gate <= 0.5 && self.last_gate > 0.5 {
                self.note_off(note_from_freq(self.inputs.frequency(i)), release_step, true);
            }
            self.last_gate = gate;

            let mut left = 0.0;
            let mut right = 0.0;
            let levels = &self.levels;
            for voice in self.voices.iter_mut() {
                if !voice.audible() {
                    continue;
                }
                if voice.active {
                    let (l, r) = voice.sample_frame();
                    let amp = voice.velocity * levels[voice.zone] * voice.release_gain;
                    left += l * amp;
                    right += r * amp;
                }
                // A stolen note keeps ringing on the same voice as it fades.
                let (fl, fr) = voice.fade_out();
                left += fl;
                right += fr;
                voice.advance();
            }
            self.outputs.set(i, left, right);
        }
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        self.inputs.block_mut(index)
    }

    fn output_block(&self, index: usize) -> &[f32] {
        self.outputs.block(index)
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    fn set_input_connected(&mut self, index: usize, connected: bool) {
        self.inputs.set_connected(index, connected);
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&self.table, &self.cells))
    }

    /// Note offs take the release in force.
    fn finish_events(&mut self) {
        if std::mem::take(&mut self.releasing) {
            let release_step = self.release_step();
            for voice in self.voices.iter_mut() {
                voice.take_release(release_step);
            }
        }
    }

    /// A note plays as it applies, in the order written, and a note off
    /// fades over the release in force as the instrument next renders. A
    /// note reads back 0, as the instrument holds no note of its own.
    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        let RtValue::F32(number) = value else {
            return Err(Refusal::Unsupported);
        };
        match self.control(control) {
            Some(InstrumentControl::Release) => {
                if !(number.is_finite() && number > 0.0) {
                    return Err(Refusal::Invalid);
                }
                self.release = number.clamp(RELEASE_RANGE.0, RELEASE_RANGE.1);
                Ok(RtValue::F32(self.release))
            }
            Some(InstrumentControl::NoteOn) => {
                let note = midi_note(number).ok_or(Refusal::Invalid)?;
                self.note_on(Note::new(note).frequency(), 1.0);
                Ok(RtValue::F32(0.0))
            }
            Some(InstrumentControl::NoteOff) => {
                let note = midi_note(number).ok_or(Refusal::Invalid)?;
                // Its fade is set as the block starts (see `process`).
                self.note_off(note, 0.0, false);
                self.releasing = true;
                Ok(RtValue::F32(0.0))
            }
            Some(InstrumentControl::Level(zone)) => {
                self.level_targets[zone] = number.clamp(0.0, 2.0);
                Ok(RtValue::F32(self.level_targets[zone]))
            }
            _ => Err(Refusal::Unsupported),
        }
    }

    /// A zone's new audio plays from its next note: a voice sounding on
    /// the old buffer finishes on it, as each latched its own clone of the
    /// cache's buffer. The zone audio it replaces is retired.
    #[allow(private_interfaces)]
    fn apply_payload(
        &mut self,
        control: ControlIndex,
        payload: Payload,
    ) -> Result<Retired, (Refusal, Payload)> {
        let Some(InstrumentControl::Asset(zone)) = self.control(control) else {
            return Err((Refusal::Unsupported, payload));
        };
        let audio = payload
            .downcast::<ZoneAudio>()
            .map_err(|payload| (Refusal::Invalid, payload))?;
        Ok(std::mem::replace(&mut self.zones[zone].audio, audio).into())
    }

    fn controls(&self) -> Vec<ControlMeta> {
        local_controls(self)
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        local_get(self, key)
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        local_set(self, key, value)
    }
}

#[cfg(test)]
mod tests;
