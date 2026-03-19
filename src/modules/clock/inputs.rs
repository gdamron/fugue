//! Input definitions for the Clock module.

pub const INPUTS: [&str; 0] = [];

pub struct ClockInputs;

impl ClockInputs {
    pub fn set(port: &str) -> Result<(), String> {
        Err(format!("Clock has no input ports, got: {}", port))
    }
}
