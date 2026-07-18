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
//!     "voices": 8,
//!     "release": 0.25,
//!     "zones": [
//!       { "root": 48, "key_range": [36, 53], "asset": "fugue.keys.grand@1.0.0:c3.wav" },
//!       { "root": 60, "key_range": [54, 65], "asset": "fugue.keys.grand@1.0.0:c4.wav",
//!         "loop": { "start_frames": 22050, "end_frames": 66150, "crossfade_frames": 2205 } },
//!       { "root": 72, "key_range": [66, 84], "asset": "fugue.keys.grand@1.0.0:c5.wav", "gain": 0.9 }
//!     ]
//!   }
//! }
//! ```
//!
//! # Gate semantics
//!
//! A note starts on a `gate` rising edge — the `frequency` and `velocity`
//! inputs are latched at that instant — and releases on the falling edge.
//! The three inputs mirror a `divisi` voice trio (`frequencyN` / `gateN` /
//! `velocityN`), so divisi can fan a line across several instances, while a
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
//! are needed (see FUG-136).
//!
//! # Inputs
//! - `frequency`: Note pitch in Hz, latched at note-on
//! - `gate`: Note gate; rising edge starts, falling edge releases
//! - `velocity`: Note level, latched at note-on (1.0 when unconnected)
//!
//! # Outputs
//! - `audio_left`, `audio_right`: Stereo mix of the voice pool

use std::any::Any;
use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::music::Note;
use crate::Module;

pub use self::controls::{LoopSpec, SampleInstrumentControls, ZoneSpec};
pub use self::voice::MAX_VOICES;

use self::controls::{NoteEvent, ZoneAudio, MAX_PENDING_NOTES};
use self::voice::{note_from_freq, resolve_zone, Voice, ZoneRuntime, DECLICK_SECONDS};

mod controls;
mod inputs;
mod outputs;
mod voice;

const DEFAULT_VOICES: usize = 8;
const DEFAULT_RELEASE: f32 = 0.1;

pub struct SampleInstrumentFactory;

impl ModuleFactory for SampleInstrumentFactory {
    fn type_id(&self) -> &'static str {
        "sample_instrument"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let (specs, voices, release) = parse_config(config)?;
        let (controls, zone_audio) = SampleInstrumentControls::new(sample_rate, release, specs)?;
        let instrument =
            SampleInstrument::new_with_controls(controls.clone(), zone_audio, voices, sample_rate);

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(instrument)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
    }
}

/// Parses the `zones` list plus pool-wide settings. A missing or null
/// config builds an empty instrument (module type discovery constructs
/// every type with a null config).
fn parse_config(config: &serde_json::Value) -> Result<(Vec<ZoneSpec>, usize, f32), String> {
    let voices = match config.get("voices") {
        None => DEFAULT_VOICES,
        Some(value) => value
            .as_u64()
            .filter(|&count| (1..=MAX_VOICES as u64).contains(&count))
            .ok_or_else(|| format!("'voices' must be an integer in 1..={}", MAX_VOICES))?
            as usize,
    };

    let release = match config.get("release") {
        None => DEFAULT_RELEASE,
        Some(value) => {
            let release = value
                .as_f64()
                .filter(|release| release.is_finite() && *release > 0.0)
                .ok_or("'release' must be a positive number of seconds")? as f32;
            release.clamp(1e-3, 30.0)
        }
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
        specs.push(parse_zone(zone, index)?);
    }
    Ok((specs, voices, release))
}

fn parse_zone(
    zone: &serde_json::Map<String, serde_json::Value>,
    index: usize,
) -> Result<ZoneSpec, String> {
    let root = zone
        .get("root")
        .and_then(|value| value.as_u64())
        .filter(|&root| root <= 127)
        .ok_or_else(|| format!("zones[{}]: 'root' must be a MIDI note number 0..=127", index))?
        as u8;

    let (key_low, key_high) = match zone.get("key_range") {
        // A zone without a range covers only its root; other notes reach it
        // through nearest-zone resolution.
        None => (root, root),
        Some(value) => {
            let range = value
                .as_array()
                .filter(|range| range.len() == 2)
                .ok_or_else(|| format!("zones[{}]: 'key_range' must be [low, high]", index))?;
            let mut bounds = range.iter().map(|bound| {
                bound.as_u64().filter(|&key| key <= 127).ok_or_else(|| {
                    format!(
                        "zones[{}]: 'key_range' bounds must be MIDI notes 0..=127",
                        index
                    )
                })
            });
            let low = bounds.next().unwrap()? as u8;
            let high = bounds.next().unwrap()? as u8;
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

    let gain = match zone.get("gain") {
        Some(value) => value
            .as_f64()
            .ok_or_else(|| format!("zones[{}]: 'gain' must be a number", index))?
            as f32,
        None => 1.0,
    };

    let loop_spec = zone
        .get("loop")
        .map(|value| parse_loop(value, index))
        .transpose()?;

    Ok(ZoneSpec {
        root,
        key_low,
        key_high,
        asset,
        gain,
        loop_spec,
    })
}

fn parse_loop(value: &serde_json::Value, index: usize) -> Result<LoopSpec, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("zones[{}]: 'loop' must be an object", index))?;
    let frames = |key: &str| {
        object.get(key).map(|value| {
            value
                .as_u64()
                .ok_or_else(|| format!("zones[{}]: loop '{}' must be a frame count", index, key))
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
    ctrl: SampleInstrumentControls,
    inputs: inputs::SampleInstrumentInputs,
    outputs: outputs::SampleInstrumentOutputs,
    zones: Vec<ZoneRuntime>,
    voices: Vec<Voice>,
    /// Per-block scratch, sized once at build: zone gains, staged swaps,
    /// and drained control-thread note events.
    gains: Vec<f32>,
    swap_scratch: Vec<Option<ZoneAudio>>,
    note_scratch: Vec<NoteEvent>,
    /// Allocation ordinal handed to the next note-on (steal-oldest order).
    next_started: u64,
    sample_rate: f32,
    /// Per-frame decay of a stolen voice's declick ramp, fixed at build.
    declick_step: f32,
    last_swaps_version: u64,
    last_notes_version: u64,
    last_gate: f32,
}

impl SampleInstrument {
    pub(crate) fn new_with_controls(
        controls: SampleInstrumentControls,
        zone_audio: Vec<ZoneAudio>,
        voices: usize,
        sample_rate: u32,
    ) -> Self {
        let zone_count = zone_audio.len();
        let zones = zone_audio
            .into_iter()
            .enumerate()
            .map(|(index, audio)| {
                let (root, key_low, key_high) = controls.zone_key(index).unwrap_or((0, 0, 0));
                ZoneRuntime {
                    audio,
                    key_low,
                    key_high,
                    root,
                    root_freq: Note::new(root).frequency(),
                }
            })
            .collect();

        Self {
            ctrl: controls,
            inputs: inputs::SampleInstrumentInputs::new(),
            outputs: outputs::SampleInstrumentOutputs::new(),
            zones,
            voices: (0..voices.clamp(1, MAX_VOICES)).map(|_| Voice::new()).collect(),
            gains: vec![1.0; zone_count],
            swap_scratch: (0..zone_count).map(|_| None).collect(),
            note_scratch: Vec::with_capacity(MAX_PENDING_NOTES),
            next_started: 0,
            sample_rate: sample_rate as f32,
            declick_step: 1.0 / (DECLICK_SECONDS * sample_rate as f32).max(1.0),
            last_swaps_version: 0,
            last_notes_version: 0,
            last_gate: 0.0,
        }
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
            let amp = voice.velocity * self.gains[voice.zone] * voice.release_gain;
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
            if !voice.held && oldest_releasing.map(|(_, s)| voice.started < s).unwrap_or(true) {
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
        // Staged zone swaps (`asset.<i>` control): the lock is taken only
        // when the version counter moved. Sounding voices latched the old
        // buffer's Arc and finish on it; new notes get the new one.
        let swaps_version = self.ctrl.swaps_version();
        if swaps_version != self.last_swaps_version {
            self.last_swaps_version = swaps_version;
            self.ctrl.take_swaps(&mut self.swap_scratch);
            for (zone, staged) in self.zones.iter_mut().zip(self.swap_scratch.iter_mut()) {
                if let Some(audio) = staged.take() {
                    zone.audio = audio;
                }
            }
        }

        // Release fade slope, from the control read once per block.
        let release_step = 1.0 / (self.ctrl.release().max(1e-3) * self.sample_rate);

        // Control-thread note events, observed once per block.
        let notes_version = self.ctrl.notes_version();
        if notes_version != self.last_notes_version {
            self.last_notes_version = notes_version;
            self.note_scratch.clear();
            self.ctrl.take_note_events(&mut self.note_scratch);
            for i in 0..self.note_scratch.len() {
                let event = self.note_scratch[i];
                if event.on {
                    self.note_on(Note::new(event.note).frequency(), 1.0);
                } else {
                    self.note_off(event.note, release_step, false);
                }
            }
        }

        // Gains are control-rate: read once per block.
        for zone in 0..self.zones.len() {
            self.gains[zone] = self.ctrl.gain(zone);
        }

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
            let gains = &self.gains;
            for voice in self.voices.iter_mut() {
                if !voice.audible() {
                    continue;
                }
                if voice.active {
                    let (l, r) = voice.sample_frame();
                    let amp = voice.velocity * gains[voice.zone] * voice.release_gain;
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
}

#[cfg(test)]
mod tests;
