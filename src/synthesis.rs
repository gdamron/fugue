use std::f32::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OscillatorType {
    Sine,
    Square,
    Sawtooth,
    Triangle,
}

pub struct Oscillator {
    osc_type: OscillatorType,
    frequency: f32,
    phase: f32,
    sample_rate: u32,
}

impl Oscillator {
    pub fn new(sample_rate: u32, osc_type: OscillatorType) -> Self {
        Self {
            osc_type,
            frequency: 440.0,
            phase: 0.0,
            sample_rate,
        }
    }

    pub fn set_frequency(&mut self, freq: f32) {
        self.frequency = freq;
    }

    pub fn set_type(&mut self, osc_type: OscillatorType) {
        self.osc_type = osc_type;
    }

    pub fn next_sample(&mut self) -> f32 {
        let sample = match self.osc_type {
            OscillatorType::Sine => (self.phase * 2.0 * PI).sin(),
            OscillatorType::Square => {
                if self.phase < 0.5 { 1.0 } else { -1.0 }
            }
            OscillatorType::Sawtooth => 2.0 * self.phase - 1.0,
            OscillatorType::Triangle => {
                4.0 * (self.phase - 0.5).abs() - 1.0
            }
        };

        self.phase += self.frequency / self.sample_rate as f32;
        self.phase %= 1.0;

        sample
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
    }
}

pub struct Filter {
    cutoff: f32,
    resonance: f32,
    prev_output: f32,
}

impl Filter {
    pub fn new() -> Self {
        Self {
            cutoff: 1000.0,
            resonance: 0.5,
            prev_output: 0.0,
        }
    }

    pub fn set_cutoff(&mut self, cutoff: f32) {
        self.cutoff = cutoff;
    }

    pub fn set_resonance(&mut self, resonance: f32) {
        self.resonance = resonance.clamp(0.0, 1.0);
    }

    pub fn process(&mut self, input: f32) -> f32 {
        let alpha = 0.1 + self.resonance * 0.5;
        self.prev_output = alpha * input + (1.0 - alpha) * self.prev_output;
        self.prev_output
    }
}
