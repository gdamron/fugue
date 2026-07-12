//! Thread-safe controls for the StepSequencer module.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::atomic::AtomicF32;
use crate::{ControlMeta, ControlSurface, ControlValue};

use super::grace::{DEFAULT_GRACE_DURATION_MS, MAX_GRACE_DURATION_MS, MIN_GRACE_DURATION_MS};
use super::{Step, DEFAULT_BASE_NOTE, DEFAULT_GATE_LENGTH, DEFAULT_STEPS};

/// Thread-safe controls for the StepSequencer module.
///
/// Controls use the uniform f32 get/set API:
/// - `base_note` - Base MIDI note (0-127)
/// - `steps` - Number of steps in pattern (1-64)
/// - `gate_length` - Default gate length ratio (0.0-1.0)
///
/// # Example
///
/// ```rust,ignore
/// let controls: StepSequencerControls = handles.get("step_sequencer.controls").unwrap();
///
/// // Adjust parameters in real-time
/// controls.set_base_note(36);
/// controls.set_steps(8);
/// controls.set_gate_length(0.75);
/// ```
#[derive(Clone)]
pub struct StepSequencerControls {
    pub(crate) base_note: Arc<Mutex<u8>>,
    pub(crate) steps: Arc<Mutex<usize>>,
    pub(crate) gate_length: Arc<Mutex<f32>>,
    pub(crate) pattern: Arc<Mutex<Vec<Step>>>,
    /// One-shot playback flag; an atomic because the audio thread reads it
    /// at sample rate (exposed as the `mode` control: loop | one_shot).
    pub(crate) one_shot: Arc<AtomicBool>,
    /// Written by the audio thread when a one-shot pattern completes (and
    /// cleared on re-arm), so live surfaces can observe the end without
    /// touching the graph. Exposed as the read-only `ended` control.
    pub(crate) ended: Arc<AtomicBool>,
    /// Duration of a single grace note in milliseconds; read once per block
    /// by the audio thread.
    pub(crate) grace_duration_ms: Arc<AtomicF32>,
    /// Grace placement (the `grace_placement` control): `false` = before the
    /// beat, `true` = on the beat.
    pub(crate) grace_on_beat: Arc<AtomicBool>,
}

impl StepSequencerControls {
    /// Creates new step sequencer controls with default values.
    pub fn new() -> Self {
        Self {
            base_note: Arc::new(Mutex::new(DEFAULT_BASE_NOTE)),
            steps: Arc::new(Mutex::new(DEFAULT_STEPS)),
            gate_length: Arc::new(Mutex::new(DEFAULT_GATE_LENGTH)),
            pattern: Arc::new(Mutex::new(Vec::new())),
            one_shot: Arc::new(AtomicBool::new(false)),
            ended: Arc::new(AtomicBool::new(false)),
            grace_duration_ms: Arc::new(AtomicF32::new(DEFAULT_GRACE_DURATION_MS)),
            grace_on_beat: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Creates new step sequencer controls with specified values.
    pub fn new_with_values(base_note: u8, steps: usize, gate_length: f32) -> Self {
        Self {
            base_note: Arc::new(Mutex::new(base_note.min(127))),
            steps: Arc::new(Mutex::new(steps.clamp(1, 64))),
            gate_length: Arc::new(Mutex::new(gate_length.clamp(0.0, 1.0))),
            pattern: Arc::new(Mutex::new(Vec::new())),
            one_shot: Arc::new(AtomicBool::new(false)),
            ended: Arc::new(AtomicBool::new(false)),
            grace_duration_ms: Arc::new(AtomicF32::new(DEFAULT_GRACE_DURATION_MS)),
            grace_on_beat: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Gets the base MIDI note.
    pub fn base_note(&self) -> u8 {
        *self.base_note.lock().unwrap()
    }

    /// Sets the base MIDI note (0-127).
    pub fn set_base_note(&self, note: u8) {
        *self.base_note.lock().unwrap() = note.min(127);
    }

    /// Gets the number of steps.
    pub fn steps(&self) -> usize {
        *self.steps.lock().unwrap()
    }

    /// Sets the number of steps (1-64).
    pub fn set_steps(&self, steps: usize) {
        *self.steps.lock().unwrap() = steps.clamp(1, 64);
    }

    /// Gets the default gate length ratio.
    pub fn gate_length(&self) -> f32 {
        *self.gate_length.lock().unwrap()
    }

    /// Sets the default gate length ratio (0.0-1.0).
    pub fn set_gate_length(&self, length: f32) {
        *self.gate_length.lock().unwrap() = length.clamp(0.0, 1.0);
    }

    /// Returns whether one-shot playback is enabled.
    pub fn one_shot(&self) -> bool {
        self.one_shot.load(Ordering::Relaxed)
    }

    /// Enables or disables one-shot playback.
    pub fn set_one_shot(&self, one_shot: bool) {
        self.one_shot.store(one_shot, Ordering::Relaxed);
    }

    /// Gets the playback mode as its control string (`loop` or `one_shot`).
    pub fn mode(&self) -> &'static str {
        if self.one_shot() {
            "one_shot"
        } else {
            "loop"
        }
    }

    /// Sets the playback mode from its control string.
    pub fn set_mode(&self, mode: &str) -> Result<(), String> {
        match mode {
            "loop" => self.set_one_shot(false),
            "one_shot" => self.set_one_shot(true),
            other => {
                return Err(format!(
                    "Unknown mode '{}' (expected loop | one_shot)",
                    other
                ))
            }
        }
        Ok(())
    }

    /// Duration of a single grace note in milliseconds.
    pub fn grace_duration_ms(&self) -> f32 {
        self.grace_duration_ms.load()
    }

    pub fn set_grace_duration_ms(&self, ms: f32) {
        self.grace_duration_ms
            .store(ms.clamp(MIN_GRACE_DURATION_MS, MAX_GRACE_DURATION_MS));
    }

    /// Whether grace chains play on the beat (delaying the principal) rather
    /// than before it.
    pub fn grace_on_beat(&self) -> bool {
        self.grace_on_beat.load(Ordering::Relaxed)
    }

    pub fn set_grace_on_beat(&self, on_beat: bool) {
        self.grace_on_beat.store(on_beat, Ordering::Relaxed);
    }

    /// Gets the grace placement as its control string (`before` or `on_beat`).
    pub fn grace_placement(&self) -> &'static str {
        if self.grace_on_beat() {
            "on_beat"
        } else {
            "before"
        }
    }

    /// Sets the grace placement from its control string.
    pub fn set_grace_placement(&self, placement: &str) -> Result<(), String> {
        match placement {
            "before" => self.set_grace_on_beat(false),
            "on_beat" => self.set_grace_on_beat(true),
            other => {
                return Err(format!(
                    "Unknown grace_placement '{}' (expected before | on_beat)",
                    other
                ))
            }
        }
        Ok(())
    }

    /// Whether a one-shot playthrough has completed (read-only; the audio
    /// thread maintains it).
    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed)
    }

    pub(crate) fn set_ended(&self, ended: bool) {
        self.ended.store(ended, Ordering::Relaxed);
    }

    /// Gets the current pattern.
    pub fn pattern(&self) -> Vec<Step> {
        self.pattern.lock().unwrap().clone()
    }

    /// Sets the current pattern.
    pub fn set_pattern(&self, pattern: Vec<Step>) {
        *self.pattern.lock().unwrap() = pattern;
    }

    /// Gets the current pattern as JSON.
    ///
    /// This is primarily used by orchestration surfaces such as `agent`, MCP,
    /// and scripts. Fugue's generic control value type does not currently have
    /// a JSON variant, so rich pattern data is exposed as a string control.
    pub fn pattern_json(&self) -> String {
        serde_json::to_string(&self.pattern()).unwrap_or_else(|_| "[]".to_string())
    }

    /// Sets the current pattern from JSON.
    ///
    /// The accepted format is the same step array accepted by the module
    /// config, for example `[{"note":0,"gate":0.5},{"note":null}]`.
    pub fn set_pattern_json(&self, value: &str) -> Result<(), String> {
        let pattern: Vec<Step> = serde_json::from_str(value).map_err(|err| err.to_string())?;
        if pattern.len() > 64 {
            return Err("pattern_json may not contain more than 64 steps".to_string());
        }
        self.set_pattern(pattern);
        Ok(())
    }
}

impl Default for StepSequencerControls {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlSurface for StepSequencerControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::number("base_note", "Base MIDI note")
                .with_range(0.0, 127.0)
                .with_default(self.base_note() as f32),
            ControlMeta::number("steps", "Number of steps in pattern")
                .with_range(1.0, 64.0)
                .with_default(self.steps() as f32),
            ControlMeta::number("gate_length", "Default gate length ratio")
                .with_range(0.0, 1.0)
                .with_default(self.gate_length()),
            ControlMeta::string("pattern_json", "Step pattern as JSON")
                .with_default(self.pattern_json()),
            ControlMeta::string(
                "mode",
                "Playback mode: loop repeats; one_shot plays once and fires the end gate",
            )
            .with_options(vec!["loop".to_string(), "one_shot".to_string()])
            .with_default(self.mode()),
            ControlMeta::number("grace_duration_ms", "Duration of a single grace note in ms")
                .with_range(MIN_GRACE_DURATION_MS, MAX_GRACE_DURATION_MS)
                .with_default(self.grace_duration_ms()),
            ControlMeta::string(
                "grace_placement",
                "Grace placement: before steals the previous step's tail; on_beat delays the principal",
            )
            .with_options(vec!["before".to_string(), "on_beat".to_string()])
            .with_default(self.grace_placement()),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "base_note" => Ok((self.base_note() as f32).into()),
            "steps" => Ok((self.steps() as f32).into()),
            "gate_length" => Ok(self.gate_length().into()),
            "pattern_json" => Ok(self.pattern_json().into()),
            "mode" => Ok(self.mode().into()),
            "grace_duration_ms" => Ok(self.grace_duration_ms().into()),
            "grace_placement" => Ok(self.grace_placement().into()),
            "ended" => Ok(self.ended().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "base_note" => self.set_base_note(value.as_number()? as u8),
            "steps" => self.set_steps(value.as_number()? as usize),
            "gate_length" => self.set_gate_length(value.as_number()?),
            "pattern_json" => self.set_pattern_json(value.as_string()?)?,
            "mode" => self.set_mode(value.as_string()?)?,
            "grace_duration_ms" => self.set_grace_duration_ms(value.as_number()?),
            "grace_placement" => self.set_grace_placement(value.as_string()?)?,
            "ended" => return Err("Control 'ended' is read-only".to_string()),
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }
}
