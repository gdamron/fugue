//! Input state for the CellSequencer module.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 6] = [
    "gate",
    "reset",
    "next_sequence",
    "previous_sequence",
    "select_sequence",
    "wait_for_cycle_end",
];

pub struct CellSequencerInputs {
    gate: [f32; MAX_BLOCK],
    reset: [f32; MAX_BLOCK],
    next_sequence: [f32; MAX_BLOCK],
    previous_sequence: [f32; MAX_BLOCK],
    select_sequence: [f32; MAX_BLOCK],
    wait_for_cycle_end: [f32; MAX_BLOCK],
    select_sequence_connected: bool,
    wait_for_cycle_end_connected: bool,
}

impl CellSequencerInputs {
    pub fn new() -> Self {
        Self {
            gate: [0.0; MAX_BLOCK],
            reset: [0.0; MAX_BLOCK],
            next_sequence: [0.0; MAX_BLOCK],
            previous_sequence: [0.0; MAX_BLOCK],
            select_sequence: [0.0; MAX_BLOCK],
            wait_for_cycle_end: [0.0; MAX_BLOCK],
            select_sequence_connected: false,
            wait_for_cycle_end_connected: false,
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "gate" => self.gate.fill(value),
            "reset" => self.reset.fill(value),
            "next_sequence" => self.next_sequence.fill(value),
            "previous_sequence" => self.previous_sequence.fill(value),
            "select_sequence" => {
                self.select_sequence.fill(value);
                self.select_sequence_connected = true;
            }
            "wait_for_cycle_end" => {
                self.wait_for_cycle_end.fill(value);
                self.wait_for_cycle_end_connected = true;
            }
            _ => return Err(format!("Unknown input port: {}", port)),
        }
        Ok(())
    }

    /// Mutable block buffer for the indexed input port. Index matches `INPUTS`.
    #[inline]
    pub fn block_mut(&mut self, index: usize) -> &mut [f32] {
        match index {
            0 => &mut self.gate,
            1 => &mut self.reset,
            2 => &mut self.next_sequence,
            3 => &mut self.previous_sequence,
            4 => &mut self.select_sequence,
            _ => &mut self.wait_for_cycle_end,
        }
    }

    /// Records whether an input port is fed by an upstream connection.
    pub fn set_connected(&mut self, index: usize, connected: bool) {
        match index {
            4 => self.select_sequence_connected = connected,
            5 => self.wait_for_cycle_end_connected = connected,
            _ => {}
        }
    }

    #[inline]
    pub fn gate(&self, i: usize) -> f32 {
        self.gate[i]
    }

    #[inline]
    pub fn reset_gate(&self, i: usize) -> f32 {
        self.reset[i]
    }

    #[inline]
    pub fn next_sequence(&self, i: usize) -> f32 {
        self.next_sequence[i]
    }

    #[inline]
    pub fn previous_sequence(&self, i: usize) -> f32 {
        self.previous_sequence[i]
    }

    #[inline]
    pub fn select_sequence(&self, i: usize, control: usize) -> usize {
        if self.select_sequence_connected {
            self.select_sequence[i].max(0.0).round() as usize
        } else {
            control
        }
    }

    #[inline]
    pub fn wait_for_cycle_end(&self, i: usize, control: bool) -> bool {
        if self.wait_for_cycle_end_connected {
            self.wait_for_cycle_end[i] > 0.5
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
