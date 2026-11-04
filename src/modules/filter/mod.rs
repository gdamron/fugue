//! Filter module for subtractive synthesis.
//!
//! The filter removes or emphasizes frequencies from the input signal,
//! which is the core of subtractive synthesis. Combined with envelope
//! modulation, filters create the classic "analog synth" sound.
//!
//! # Filter Types
//!
//! - **Low-pass (LPF)** - Removes frequencies above cutoff, warm/dark sound
//! - **High-pass (HPF)** - Removes frequencies below cutoff, thin/bright sound
//! - **Band-pass (BPF)** - Passes frequencies around cutoff, vocal/nasal sound
//! - **Bell** - Boosts or cuts `boost` dB around the cutoff, `q` wide
//! - **Low shelf / high shelf** - Boost or cut everything below / above it
//! - **Notch** - Removes the cutoff, `q` wide
//!
//! The EQ responses (bell, shelves, notch) follow the RBJ cookbook's
//! magnitudes and ignore `resonance`; lowpass, highpass and bandpass ignore
//! `boost` and `q`, and can run at 12 or 24 dB per octave (`slope`). Any
//! response can be bypassed, crossfading to the input.
//!
//! # Features
//!
//! - Cutoff frequency: 20 Hz to 20 kHz
//! - Resonance (Q): 0.0 to 1.0 (self-oscillates near 1.0)
//! - CV inputs for cutoff and resonance modulation
//! - State-variable filter design for stability and musicality
//!
//! # Example Invention
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "osc", "type": "oscillator", "config": { "waveform": "sawtooth" } },
//!     { "id": "filter", "type": "filter", "config": { "filter_type": "lowpass", "cutoff": 1000.0, "resonance": 0.5 } },
//!     { "id": "env", "type": "adsr", "config": { "attack": 0.01, "decay": 0.3, "sustain": 0.2, "release": 0.5 } }
//!   ],
//!   "connections": [
//!     { "from": "osc", "from_port": "audio", "to": "filter", "to_port": "audio" },
//!     { "from": "env", "from_port": "envelope", "to": "filter", "to_port": "cutoff_mod" }
//!   ]
//! }
//! ```

use std::f32::consts::PI;
use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::traits::ControlMeta;
use crate::Module;

use self::controls::{
    BOOST, BYPASS, CUTOFF, CUTOFF_MOD_DEPTH, DEFAULT_Q, FILTER_TYPE, MAX_BOOST, MAX_Q, MIN_Q, Q,
    RESONANCE, SLOPE, TABLE,
};
use self::fade::Crossfade;
use self::shape::{Coefficients, EqDesign, Stage};

mod controls;
mod factory;
mod fade;
mod glide;
mod inputs;
mod outputs;
mod shape;
mod types;

pub use self::factory::FilterFactory;
pub use self::fade::FADE_MS;
pub use self::glide::GLIDE_MS;
pub use self::types::FilterType;

/// The damping of the second stage a `24db` slope adds: Butterworth, with
/// no peak of its own, so `resonance` shapes one peak rather than squaring
/// it (two resonant stages in series would ring far louder than either).
const SECOND_STAGE_DAMPING: f32 = std::f32::consts::SQRT_2;

/// Polynomial approximation of tan(x) for x in [0, ~0.85].
///
/// Uses the degree-7 Taylor series in Horner form. Max error < 0.2% for
/// x < 0.75 rad (cutoff < 10.5 kHz at 44.1 kHz), which is inaudible for
/// filter coefficient calculations. Roughly 10× faster than libm tan().
/// Above that it falls short (a 20 kHz cutoff sounds near 19 kHz at
/// 48 kHz), but lowpass, highpass and bandpass keep it so existing
/// inventions sound as they always have; the EQ responses use the exact
/// [`crate::dsp::tan_quarter_turn`].
#[inline(always)]
fn fast_tan(x: f32) -> f32 {
    let x2 = x * x;
    x * (1.0 + x2 * (0.333_333_34 + x2 * (0.133_333_34 + x2 * 0.053_968_26)))
}

/// A resonant filter for subtractive synthesis, and the bands of an EQ.
///
/// Uses a state-variable filter (SVF) topology, which provides excellent
/// stability and can produce low-pass, high-pass, and band-pass outputs
/// simultaneously; the EQ responses mix those taps with the input.
///
/// # Inputs
///
/// - `audio` - Audio signal to filter
/// - `cutoff` - Cutoff frequency in Hz (overrides control if connected)
/// - `cutoff_mod` - Cutoff modulation (scaled by cutoff_mod_depth, in Hz)
/// - `resonance` - Resonance modulation (0.0 to 1.0)
///
/// # Outputs
///
/// - `audio` - Filtered audio signal
///
/// # Controls
///
/// - `cutoff` - Base cutoff frequency in Hz (default: 1000.0)
/// - `resonance` - Resonance/Q 0.0-1.0 (default: 0.0)
/// - `filter_type` - lowpass, highpass, bandpass, bell, lowshelf, highshelf
///   or notch
/// - `cutoff_mod_depth` - CV modulation depth in Hz (default: 5000.0)
/// - `boost` - EQ gain in dB, -24 to 24 (default: 0)
/// - `q` - EQ width or steepness, 0.1 to 18 (default: 0.7071)
/// - `bypass` - pass the input through (default: false)
/// - `slope` - 12 or 24 dB per octave for lowpass, highpass and bandpass
pub struct Filter {
    sample_rate: u32,

    // Controls, applied on the thread running the filter.
    cutoff: f32,
    resonance: f32,
    filter_type: FilterType,
    cutoff_mod_depth: f32,
    boost: f32,
    q: f32,
    cells: Arc<ControlCells>,
    // Where cutoff, resonance, boost and q writes are on their way.
    glide: glide::ControlGlide,
    // How much of the filtered signal is heard (bypass), and of the
    // second stage (slope).
    engaged: Crossfade,
    steep: Crossfade,

    stage: Stage,
    second_stage: Stage,

    // Signal inputs
    inputs: inputs::FilterInputs,

    // Cached output
    outputs: outputs::FilterOutputs,

    // Cached lowpass/highpass/bandpass coefficients, recomputed only when
    // the effective cutoff or resonance changes. Sentinel -1.0 forces
    // computation on first call.
    coeff_cutoff: f32,
    coeff_resonance: f32,
    classic: Coefficients,
    classic_second: Coefficients,
    // The EQ responses' coefficients, recomputed only on change.
    eq: EqDesign,
}

impl Filter {
    /// Creates a new filter with default controls.
    pub fn new(sample_rate: u32) -> Self {
        Self::with_cells(
            sample_rate,
            Arc::new(ControlCells::new(controls::defaults())),
        )
    }

    /// A filter holding what `cells` hold, applied (so clamped).
    fn with_cells(sample_rate: u32, cells: Arc<ControlCells>) -> Self {
        let mut filter = Self {
            sample_rate,
            cutoff: 1000.0,
            resonance: 0.0,
            filter_type: FilterType::LowPass,
            cutoff_mod_depth: 5000.0,
            boost: 0.0,
            q: DEFAULT_Q,
            cells,
            glide: glide::ControlGlide::new(sample_rate, 1000.0, 0.0),
            engaged: Crossfade::new(sample_rate, true),
            steep: Crossfade::new(sample_rate, false),
            stage: Stage::default(),
            second_stage: Stage::default(),
            inputs: inputs::FilterInputs::new(),
            outputs: outputs::FilterOutputs::new(),
            coeff_cutoff: -1.0,
            coeff_resonance: -1.0,
            classic: Coefficients::default(),
            classic_second: Coefficients::default(),
            eq: EqDesign::new(sample_rate),
        };
        for index in 0..TABLE.len() {
            let index = ControlIndex(index as u16);
            if let Some(value) = filter.cells.load(index) {
                let _ = apply_declared(&mut filter, index, value);
            }
        }
        filter
    }
    /// Sets the filter type (legacy API).
    pub fn with_filter_type(mut self, filter_type: FilterType) -> Self {
        self.set_filter_type(filter_type);
        self
    }

    /// Sets the cutoff frequency in Hz (legacy API).
    pub fn with_cutoff(mut self, cutoff: f32) -> Self {
        self.set_cutoff(cutoff);
        self
    }

    /// Sets the resonance (legacy API).
    pub fn with_resonance(mut self, resonance: f32) -> Self {
        self.set_resonance(resonance);
        self
    }

    /// Sets the CV modulation amount in Hz (legacy API).
    pub fn with_cutoff_mod_depth(mut self, amount: f32) -> Self {
        let _ = apply_declared(&mut self, CUTOFF_MOD_DEPTH, RtValue::F32(amount));
        self
    }

    /// Sets the filter type (legacy API).
    pub fn set_filter_type(&mut self, filter_type: FilterType) {
        let _ = apply_declared(self, FILTER_TYPE, RtValue::U32(filter_type.position()));
    }

    /// Sets the cutoff frequency in Hz (legacy API).
    pub fn set_cutoff(&mut self, cutoff: f32) {
        let _ = apply_declared(self, CUTOFF, RtValue::F32(cutoff));
    }

    /// Sets the resonance (legacy API).
    pub fn set_resonance(&mut self, resonance: f32) {
        let _ = apply_declared(self, RESONANCE, RtValue::F32(resonance));
    }

    /// The effective cutoff for frame `i`: the control or input, plus the
    /// modulation, in range.
    #[inline(always)]
    fn cutoff_at(&self, i: usize, control_cutoff: f32, cutoff_mod_depth: f32) -> f32 {
        let base_cutoff = self.inputs.cutoff(i, control_cutoff);
        (base_cutoff + self.inputs.cutoff_mod(i) * cutoff_mod_depth).clamp(20.0, 20000.0)
    }

    /// Processes one sample through the lowpass, highpass or bandpass
    /// response using the trapezoidal integrated SVF (Andrew Simper /
    /// Cytomic). Unconditionally stable at all frequencies.
    #[inline(always)]
    fn classic_sample(
        &mut self,
        i: usize,
        control_cutoff: f32,
        control_resonance: f32,
        cutoff_mod_depth: f32,
        filter_type: FilterType,
    ) -> f32 {
        let effective_cutoff = self.cutoff_at(i, control_cutoff, cutoff_mod_depth);
        let base_resonance = self.inputs.resonance(i, control_resonance);
        let effective_resonance = base_resonance.clamp(0.0, 0.99);

        // Recompute SVF coefficients only when cutoff or resonance changes.
        // During ADSR sustain or slow LFO phases the values are stable and we
        // skip the fast_tan() call entirely.
        if effective_cutoff != self.coeff_cutoff || effective_resonance != self.coeff_resonance {
            let g = fast_tan(PI * effective_cutoff / self.sample_rate as f32);
            let k = 2.0 * (1.0 - effective_resonance);
            self.classic = Coefficients::stage(g, k);
            self.classic_second = Coefficients::stage(g, SECOND_STAGE_DAMPING);
            self.coeff_cutoff = effective_cutoff;
            self.coeff_resonance = effective_resonance;
        }

        let input = self.inputs.audio(i);
        let first = self.stage.classic(&self.classic, filter_type, input);
        if !self.steep.is_live() {
            return first;
        }
        let second = self
            .second_stage
            .classic(&self.classic_second, filter_type, first);
        self.steep.mix(first, second)
    }

    /// Processes one sample through an EQ response.
    #[inline(always)]
    fn eq_sample(
        &mut self,
        i: usize,
        control_cutoff: f32,
        cutoff_mod_depth: f32,
        filter_type: FilterType,
    ) -> f32 {
        let effective_cutoff = self.cutoff_at(i, control_cutoff, cutoff_mod_depth);
        let boost = self.glide.next_boost(self.boost);
        let q = self.glide.next_q(self.q);
        self.eq.design(filter_type, effective_cutoff, boost, q);
        let input = self.inputs.audio(i);
        let coefficients = &self.eq.coefficients;
        let (bandpass, lowpass) = self.stage.tick(coefficients, input);
        coefficients.mix(input, bandpass, lowpass)
    }

    /// Crossfades the block's output towards its input as the filter is
    /// bypassed: untouched while engaged, the input itself once bypassed.
    fn apply_bypass(&mut self, frames: usize) {
        if !self.engaged.is_fading() && self.engaged.is_on() {
            return;
        }
        for i in 0..frames {
            let mixed = self
                .engaged
                .mix(self.inputs.audio(i), self.outputs.value(i));
            self.outputs.set(i, mixed);
        }
    }

    /// Resets the filter state.
    pub fn reset(&mut self) {
        self.stage.reset();
        self.second_stage.reset();
    }
}

impl Module for Filter {
    fn name(&self) -> &str {
        "Filter"
    }

    fn process(&mut self, frames: usize) -> bool {
        // Cutoff and resonance controls hold still except while a write
        // glides in (see `glide`). Cutoff, cutoff CV, and resonance signal
        // inputs remain audio-rate, and feedback groups call process(1).
        let cutoff_connected = self.inputs.cutoff_connected();
        let resonance_connected = self.inputs.resonance_connected();
        let cutoff_mod_depth = self.cutoff_mod_depth;
        let filter_type = self.filter_type;

        if filter_type.is_eq() {
            for i in 0..frames {
                let cutoff = self.glide.next_cutoff(self.cutoff);
                let cutoff = if cutoff_connected { 0.0 } else { cutoff };
                let audio = self.eq_sample(i, cutoff, cutoff_mod_depth, filter_type);
                self.outputs.set(i, audio);
            }
        } else {
            let mut i = 0;
            while i < frames {
                let cutoff = self.glide.next_cutoff(self.cutoff);
                let resonance = self.glide.next_resonance(self.resonance);
                let audio = self.classic_sample(
                    i,
                    if cutoff_connected { 0.0 } else { cutoff },
                    if resonance_connected { 0.0 } else { resonance },
                    cutoff_mod_depth,
                    filter_type,
                );
                self.outputs.set(i, audio);
                i += 1;
            }
        }
        self.apply_bypass(frames);
        if frames > 0 {
            self.glide.heard();
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
        Some((&TABLE, &self.cells))
    }

    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        let applied = match (control, value) {
            (CUTOFF, RtValue::F32(cutoff)) => {
                self.cutoff = cutoff.clamp(20.0, 20000.0);
                self.glide.cutoff_to(self.cutoff);
                RtValue::F32(self.cutoff)
            }
            (RESONANCE, RtValue::F32(resonance)) => {
                self.resonance = resonance.clamp(0.0, 1.0);
                self.glide.resonance_to(self.resonance);
                RtValue::F32(self.resonance)
            }
            (FILTER_TYPE, RtValue::U32(position)) => {
                self.filter_type = FilterType::from_position(position).ok_or(Refusal::Invalid)?;
                value
            }
            (CUTOFF_MOD_DEPTH, RtValue::F32(depth)) => {
                self.cutoff_mod_depth = depth.max(0.0);
                RtValue::F32(self.cutoff_mod_depth)
            }
            (BOOST, RtValue::F32(boost)) => {
                self.boost = boost.clamp(-MAX_BOOST, MAX_BOOST);
                self.glide.boost_to(self.boost);
                RtValue::F32(self.boost)
            }
            (Q, RtValue::F32(q)) => {
                self.q = q.clamp(MIN_Q, MAX_Q);
                self.glide.q_to(self.q);
                RtValue::F32(self.q)
            }
            (BYPASS, RtValue::Bool(bypass)) => {
                self.engaged.switch(!bypass, self.glide.is_heard());
                value
            }
            (SLOPE, RtValue::U32(position)) => {
                let steep = match position {
                    0 => false,
                    1 => true,
                    _ => return Err(Refusal::Invalid),
                };
                if steep && !self.steep.is_live() {
                    // Starts from rest rather than from long-stale state.
                    self.second_stage.reset();
                }
                self.steep.switch(steep, self.glide.is_heard());
                value
            }
            _ => return Err(Refusal::Unsupported),
        };
        Ok(applied)
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
mod eq_tests;
#[cfg(test)]
mod glide_tests;
#[cfg(test)]
mod tests;
