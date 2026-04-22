//! Input state for the CellSequencer module.

pub const INPUTS: [&str; 6] = [
    "gate",
    "reset",
    "next_sequence",
    "previous_sequence",
    "select_sequence",
    "wait_for_cycle_end",
];

pub struct CellSequencerInputs {
    gate: f32,
    reset: f32,
    next_sequence: f32,
    previous_sequence: f32,
    select_sequence: f32,
    wait_for_cycle_end: f32,
    select_sequence_active: bool,
    wait_for_cycle_end_active: bool,
}

impl CellSequencerInputs {
    pub fn new() -> Self {
        Self {
            gate: 0.0,
            reset: 0.0,
            next_sequence: 0.0,
            previous_sequence: 0.0,
            select_sequence: 0.0,
            wait_for_cycle_end: 0.0,
            select_sequence_active: false,
            wait_for_cycle_end_active: false,
        }
    }

    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "gate" => self.gate = value,
            "reset" => self.reset = value,
            "next_sequence" => self.next_sequence = value,
            "previous_sequence" => self.previous_sequence = value,
            "select_sequence" => {
                self.select_sequence = value;
                self.select_sequence_active = true;
            }
            "wait_for_cycle_end" => {
                self.wait_for_cycle_end = value;
                self.wait_for_cycle_end_active = true;
            }
            _ => return Err(format!("Unknown input port: {}", port)),
        }
        Ok(())
    }

    pub fn reset(&mut self) {
        self.select_sequence_active = false;
        self.wait_for_cycle_end_active = false;
    }

    pub fn gate(&self) -> f32 {
        self.gate
    }

    pub fn reset_gate(&self) -> f32 {
        self.reset
    }

    pub fn next_sequence(&self) -> f32 {
        self.next_sequence
    }

    pub fn previous_sequence(&self) -> f32 {
        self.previous_sequence
    }

    pub fn select_sequence(&self, control: usize) -> usize {
        if self.select_sequence_active {
            self.select_sequence.max(0.0).round() as usize
        } else {
            control
        }
    }

    pub fn wait_for_cycle_end(&self, control: bool) -> bool {
        if self.wait_for_cycle_end_active {
            self.wait_for_cycle_end > 0.5
        } else {
            control
        }
    }
}

impl Default for CellSequencerInputs {
    fn default() -> Self {
        Self::new()
    }
}
