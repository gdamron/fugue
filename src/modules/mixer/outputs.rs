//! Output definitions for the Mixer module.

pub const OUTPUTS: [&str; 2] = ["left", "right"];

pub struct MixerOutputs;

impl MixerOutputs {
    pub fn get(port: &str, left: f32, right: f32) -> Result<f32, String> {
        match port {
            "left" => Ok(left),
            "right" => Ok(right),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}
