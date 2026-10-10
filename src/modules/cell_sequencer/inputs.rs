//! Input state for the CellSequencer module.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 6] = [
    "clock",
    "reset",
    "next_cell",
    "previous_cell",
    "select_cell",
    "wait_for_cycle_end",
];

pub struct CellSequencerInputs {
    clock: [f32; MAX_BLOCK],
    reset: [f32; MAX_BLOCK],
    next_cell: [f32; MAX_BLOCK],
    previous_cell: [f32; MAX_BLOCK],
    select_cell: [f32; MAX_BLOCK],
    wait_for_cycle_end: [f32; MAX_BLOCK],
    select_cell_connected: bool,
    wait_for_cycle_end_connected: bool,
}

impl CellSequencerInputs {
    pub fn new() -> Self {
        Self {
            clock: [0.0; MAX_BLOCK],
            reset: [0.0; MAX_BLOCK],
            next_cell: [0.0; MAX_BLOCK],
            previous_cell: [0.0; MAX_BLOCK],
            select_cell: [0.0; MAX_BLOCK],
            wait_for_cycle_end: [0.0; MAX_BLOCK],
            select_cell_connected: false,
            wait_for_cycle_end_connected: false,
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "clock" => self.clock.fill(value),
            "reset" => self.reset.fill(value),
            "next_cell" => self.next_cell.fill(value),
            "previous_cell" => self.previous_cell.fill(value),
            "select_cell" => {
                self.select_cell.fill(value);
                self.select_cell_connected = true;
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
            0 => &mut self.clock,
            1 => &mut self.reset,
            2 => &mut self.next_cell,
            3 => &mut self.previous_cell,
            4 => &mut self.select_cell,
            _ => &mut self.wait_for_cycle_end,
        }
    }

    /// Records whether an input port is fed by an upstream connection.
    pub fn set_connected(&mut self, index: usize, connected: bool) {
        match index {
            4 => self.select_cell_connected = connected,
            5 => self.wait_for_cycle_end_connected = connected,
            _ => {}
        }
    }

    #[inline]
    pub fn clock(&self, i: usize) -> f32 {
        self.clock[i]
    }

    #[inline]
    pub fn reset_gate(&self, i: usize) -> f32 {
        self.reset[i]
    }

    #[inline]
    pub fn next_cell(&self, i: usize) -> f32 {
        self.next_cell[i]
    }

    #[inline]
    pub fn previous_cell(&self, i: usize) -> f32 {
        self.previous_cell[i]
    }

    #[inline]
    pub fn select_cell(&self, i: usize, control: usize) -> usize {
        if self.select_cell_connected {
            self.select_cell[i].max(0.0).round() as usize
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
