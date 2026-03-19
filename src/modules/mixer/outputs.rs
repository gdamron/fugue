//! Output definitions for the Mixer module.

pub const OUTPUTS: [&str; 1] = ["out"];

pub struct MixerOutputs;

impl MixerOutputs {
    pub fn get(port: &str, out: f32) -> Result<f32, String> {
        match port {
            "out" => Ok(out),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}
