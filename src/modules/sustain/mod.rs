//! Sustain-pedal gate module.
//!
//! Models a piano's damper pedal as a pure gate transformation, so pedal
//! behavior composes with any gate-consuming module instead of living inside
//! a particular envelope:
//!
//! - While the key is down (`gate` high), the output follows it.
//! - If the pedal is down when the key comes up, the dampers stay off: the
//!   output gate holds high and the note keeps evolving however its envelope
//!   evolves — a struck voice (envelope sustain level 0) rings out on its
//!   natural decay, a sustaining voice holds its plateau.
//! - When the pedal comes up with the key already up, the output falls and
//!   the envelope's release is the damper landing on the string.
//! - A re-strike while the pedal is holding the note forces the output low
//!   for exactly one sample, so a downstream envelope sees a fresh rising
//!   edge and retriggers. (A pedal pressed *after* a note released does not
//!   re-catch it.)
//!
//! # Inputs
//! - `gate`: The note gate (key down / key up)
//! - `pedal`: Sustain-pedal gate (>0.0 = dampers up)
//!
//! # Outputs
//! - `gate`: The transformed gate, suitable for an envelope's `gate` input
//!
//! # Example
//!
//! ```rust,ignore
//! // Inside a voice development: gate -> sustain -> envelope.
//! {
//!   "connections": [
//!     { "from": "sus", "from_port": "gate", "to": "env", "to_port": "gate" }
//!   ],
//!   "inputs": [
//!     { "name": "gate", "to": "sus", "to_port": "gate" },
//!     { "name": "pedal", "to": "sus", "to_port": "pedal" }
//!   ]
//! }
//! ```

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::{Module, MAX_BLOCK};

const INPUTS: [&str; 2] = ["gate", "pedal"];
const OUTPUTS: [&str; 1] = ["gate"];

/// Factory for constructing sustain modules. Takes no configuration.
pub struct SustainFactory;

impl ModuleFactory for SustainFactory {
    fn type_id(&self) -> &'static str {
        "sustain"
    }

    fn build(
        &self,
        _sample_rate: u32,
        _config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(Sustain::new())),
            handles: Vec::new(),
            control_surface: None,
            sink: None,
        })
    }
}

/// Sustain-pedal gate transform. See the module docs.
pub struct Sustain {
    gate_in: [f32; MAX_BLOCK],
    pedal_in: [f32; MAX_BLOCK],
    gate_out: [f32; MAX_BLOCK],
    /// The key has come up while the pedal was down: the note is being held
    /// by the pedal. Cleared by a re-strike or by the pedal coming up.
    held: bool,
    last_gate_high: bool,
}

impl Sustain {
    pub fn new() -> Self {
        Self {
            gate_in: [0.0; MAX_BLOCK],
            pedal_in: [0.0; MAX_BLOCK],
            gate_out: [0.0; MAX_BLOCK],
            held: false,
            last_gate_high: false,
        }
    }
}

impl Default for Sustain {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for Sustain {
    fn name(&self) -> &str {
        "Sustain"
    }

    fn process(&mut self, frames: usize) -> bool {
        for i in 0..frames {
            let gate_high = self.gate_in[i] > 0.0;
            let pedal_high = self.pedal_in[i] > 0.0;
            let gate_triggered = gate_high && !self.last_gate_high;
            let gate_released = !gate_high && self.last_gate_high;

            // A re-strike while the pedal is holding the note: force one
            // sample low so the downstream envelope retriggers.
            let dip = gate_triggered && self.held;
            if gate_triggered {
                self.held = false;
            }
            if gate_released && pedal_high {
                self.held = true;
            }
            if !pedal_high {
                // Pedal up (or never down): nothing is held. A note whose
                // key is still down is unaffected; a pedal-held note falls
                // to its release.
                self.held = false;
            }

            self.gate_out[i] = if dip || !(gate_high || self.held) {
                0.0
            } else {
                1.0
            };
            self.last_gate_high = gate_high;
        }
        true
    }

    fn inputs(&self) -> &[&str] {
        &INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &OUTPUTS
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        match index {
            0 => &mut self.gate_in,
            _ => &mut self.pedal_in,
        }
    }

    fn output_block(&self, _index: usize) -> &[f32] {
        &self.gate_out
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "gate" => {
                self.gate_in.fill(value);
                Ok(())
            }
            "pedal" => {
                self.pedal_in.fill(value);
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "gate" => Ok(self.gate_out[0]),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(sustain: &mut Sustain, samples: usize) {
        for _ in 0..samples {
            sustain.process(1);
        }
    }

    #[test]
    fn passes_gate_through_without_pedal() {
        let mut sustain = Sustain::new();
        sustain.set_input("gate", 1.0).unwrap();
        run(&mut sustain, 3);
        assert_eq!(sustain.get_output("gate").unwrap(), 1.0);
        sustain.set_input("gate", 0.0).unwrap();
        run(&mut sustain, 1);
        assert_eq!(sustain.get_output("gate").unwrap(), 0.0);
    }

    #[test]
    fn pedal_holds_gate_past_key_up() {
        let mut sustain = Sustain::new();
        sustain.set_input("pedal", 1.0).unwrap();
        sustain.set_input("gate", 1.0).unwrap();
        run(&mut sustain, 3);
        sustain.set_input("gate", 0.0).unwrap();
        run(&mut sustain, 10);
        assert_eq!(
            sustain.get_output("gate").unwrap(),
            1.0,
            "pedal-held note keeps its gate open"
        );

        // Pedal up with the key up: the gate falls.
        sustain.set_input("pedal", 0.0).unwrap();
        run(&mut sustain, 1);
        assert_eq!(sustain.get_output("gate").unwrap(), 0.0);
    }

    #[test]
    fn pedal_cycle_does_not_disturb_held_key() {
        let mut sustain = Sustain::new();
        sustain.set_input("gate", 1.0).unwrap();
        run(&mut sustain, 2);
        sustain.set_input("pedal", 1.0).unwrap();
        run(&mut sustain, 2);
        sustain.set_input("pedal", 0.0).unwrap();
        run(&mut sustain, 2);
        assert_eq!(
            sustain.get_output("gate").unwrap(),
            1.0,
            "a key that is still down is unaffected by the pedal"
        );
    }

    #[test]
    fn restrike_while_held_dips_for_one_sample() {
        let mut sustain = Sustain::new();
        sustain.set_input("pedal", 1.0).unwrap();
        sustain.set_input("gate", 1.0).unwrap();
        run(&mut sustain, 2);
        sustain.set_input("gate", 0.0).unwrap();
        run(&mut sustain, 2);
        assert_eq!(sustain.get_output("gate").unwrap(), 1.0, "held by pedal");

        // Re-strike: exactly one low sample, then high again.
        sustain.set_input("gate", 1.0).unwrap();
        sustain.process(1);
        assert_eq!(
            sustain.get_output("gate").unwrap(),
            0.0,
            "re-strike forces a one-sample release edge"
        );
        sustain.process(1);
        assert_eq!(sustain.get_output("gate").unwrap(), 1.0);
    }

    #[test]
    fn pedal_after_release_does_not_recatch() {
        let mut sustain = Sustain::new();
        sustain.set_input("gate", 1.0).unwrap();
        run(&mut sustain, 2);
        sustain.set_input("gate", 0.0).unwrap();
        run(&mut sustain, 2);
        assert_eq!(sustain.get_output("gate").unwrap(), 0.0);

        // Pressing the pedal after the note released does not bring it back.
        sustain.set_input("pedal", 1.0).unwrap();
        run(&mut sustain, 2);
        assert_eq!(sustain.get_output("gate").unwrap(), 0.0);
    }

    #[test]
    fn invalid_ports_error() {
        let mut sustain = Sustain::new();
        assert!(sustain.set_input("invalid", 1.0).is_err());
        assert!(sustain.get_output("invalid").is_err());
    }
}
