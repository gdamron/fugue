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

use self::controls::{CUTOFF, CUTOFF_MOD_DEPTH, FILTER_TYPE, RESONANCE, TABLE};
use self::shape::{Coefficients, Stage};

mod controls;
mod factory;
mod glide;
mod inputs;
mod outputs;
mod shape;

pub use self::factory::FilterFactory;
pub use self::glide::GLIDE_MS;

/// Filter type selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterType {
    /// Low-pass filter - removes frequencies above cutoff.
    #[default]
    LowPass,
    /// High-pass filter - removes frequencies below cutoff.
    HighPass,
    /// Band-pass filter - passes frequencies around cutoff.
    BandPass,
}

impl FilterType {
    /// Its position among the `filter_type` options.
    fn position(self) -> u32 {
        match self {
            FilterType::LowPass => 0,
            FilterType::HighPass => 1,
            FilterType::BandPass => 2,
        }
    }

    /// The type at `position` among the `filter_type` options.
    fn from_position(position: u32) -> Option<Self> {
        match position {
            0 => Some(FilterType::LowPass),
            1 => Some(FilterType::HighPass),
            2 => Some(FilterType::BandPass),
            _ => None,
        }
    }
}

/// Polynomial approximation of tan(x) for x in [0, ~0.85].
///
/// Uses the degree-7 Taylor series in Horner form. Max error < 0.2% for
/// x < 0.75 rad (cutoff < 10.5 kHz at 44.1 kHz), which is inaudible for
/// filter coefficient calculations. Roughly 10× faster than libm tan().
#[inline(always)]
fn fast_tan(x: f32) -> f32 {
    let x2 = x * x;
    x * (1.0 + x2 * (0.333_333_34 + x2 * (0.133_333_34 + x2 * 0.053_968_26)))
}

/// A resonant filter for subtractive synthesis.
///
/// Uses a state-variable filter (SVF) topology, which provides excellent
/// stability and can produce low-pass, high-pass, and band-pass outputs
/// simultaneously.
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
/// - `filter_type` - Filter type (0=LowPass, 1=HighPass, 2=BandPass)
/// - `cutoff_mod_depth` - CV modulation depth in Hz (default: 5000.0)
pub struct Filter {
    sample_rate: u32,

    // Controls, applied on the thread running the filter.
    cutoff: f32,
    resonance: f32,
    filter_type: FilterType,
    cutoff_mod_depth: f32,
    cells: Arc<ControlCells>,
    // Where cutoff and resonance writes are on their way to the controls.
    glide: glide::ControlGlide,

    // TPT SVF integrator state
    stage: Stage,

    // Signal inputs
    inputs: inputs::FilterInputs,

    // Cached output
    outputs: outputs::FilterOutputs,

    // Cached SVF coefficients — recomputed only when effective cutoff or
    // resonance changes. Sentinel -1.0 forces computation on first call.
    coeff_cutoff: f32,
    coeff_resonance: f32,
    classic: Coefficients,
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
            cells,
            glide: glide::ControlGlide::new(sample_rate, 1000.0, 0.0),
            stage: Stage::default(),
            inputs: inputs::FilterInputs::new(),
            outputs: outputs::FilterOutputs::new(),
            coeff_cutoff: -1.0,
            coeff_resonance: -1.0,
            classic: Coefficients::default(),
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

    /// Processes one sample through the filter using the trapezoidal integrated
    /// SVF (Andrew Simper / Cytomic). Unconditionally stable at all frequencies.
    #[inline(always)]
    fn process_sample_with(
        &mut self,
        i: usize,
        control_cutoff: f32,
        control_resonance: f32,
        cutoff_mod_depth: f32,
        filter_type: FilterType,
    ) -> f32 {
        let base_cutoff = self.inputs.cutoff(i, control_cutoff);
        let base_resonance = self.inputs.resonance(i, control_resonance);
        let effective_cutoff =
            (base_cutoff + self.inputs.cutoff_mod(i) * cutoff_mod_depth).clamp(20.0, 20000.0);
        let effective_resonance = base_resonance.clamp(0.0, 0.99);

        // Recompute SVF coefficients only when cutoff or resonance changes.
        // During ADSR sustain or slow LFO phases the values are stable and we
        // skip the fast_tan() call entirely.
        if effective_cutoff != self.coeff_cutoff || effective_resonance != self.coeff_resonance {
            let g = fast_tan(PI * effective_cutoff / self.sample_rate as f32);
            let k = 2.0 * (1.0 - effective_resonance);
            self.classic = Coefficients::stage(g, k);
            self.coeff_cutoff = effective_cutoff;
            self.coeff_resonance = effective_resonance;
        }

        let input = self.inputs.audio(i);
        self.stage.classic(&self.classic, filter_type, input)
    }

    /// Resets the filter state.
    pub fn reset(&mut self) {
        self.stage.reset();
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

        let mut i = 0;
        while i < frames {
            let cutoff = self.glide.next_cutoff(self.cutoff);
            let resonance = self.glide.next_resonance(self.resonance);
            let audio = self.process_sample_with(
                i,
                if cutoff_connected { 0.0 } else { cutoff },
                if resonance_connected { 0.0 } else { resonance },
                cutoff_mod_depth,
                filter_type,
            );
            self.outputs.set(i, audio);
            i += 1;
        }
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
mod glide_tests;
#[cfg(test)]
mod tests;
