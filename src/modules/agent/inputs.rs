//! Input definitions for the Agent module.

pub const INPUTS: [&str; 2] = ["trigger", "reset"];

#[derive(Debug, Clone, Copy)]
pub struct AgentInputs {
    trigger: f32,
    reset: f32,
}

impl AgentInputs {
    pub fn new() -> Self {
        Self {
            trigger: 0.0,
            reset: 0.0,
        }
    }

    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "trigger" => {
                self.trigger = value;
                Ok(())
            }
            "reset" => {
                self.reset = value;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    pub fn trigger(&self) -> f32 {
        self.trigger
    }

    pub fn reset(&self) -> f32 {
        self.reset
    }
}
