//! Output state for the Adsr module.

pub const OUTPUTS: [&str; 1] = ["envelope"];

pub struct AdsrOutputs;

impl AdsrOutputs {
    pub fn get(port: &str, envelope: f32) -> Result<f32, String> {
        match port {
            "envelope" => Ok(envelope.clamp(0.0, 1.0)),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}
