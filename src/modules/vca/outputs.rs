//! Output definitions for the Vca module.

pub const OUTPUTS: [&str; 1] = ["audio"];

pub struct VcaOutputs;

impl VcaOutputs {
    pub fn get(port: &str, audio: f32) -> Result<f32, String> {
        match port {
            "audio" => Ok(audio),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}
