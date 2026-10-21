//! A cell sequencer's legacy controls as a scheduler writes them, on the
//! audio thread (see [`crate::ControlSurface::set_scheduled`]): atomics
//! only. `select_cell` clamps against the bank's length, packed with the
//! selection, rather than locking the bank for it, so no write waits or is
//! refused as busy, and a bank replaced meanwhile lands before or after it.

use super::CellSequencerControls;
use crate::control_request::{scheduled_bool, scheduled_number, LegacyRefusal, TableValue};

impl CellSequencerControls {
    /// What `set_control` writes of a number or a boolean: the strings and
    /// the read-only controls are refused.
    pub(super) fn scheduled_write(
        &self,
        key: &str,
        value: TableValue,
    ) -> Result<(), LegacyRefusal> {
        match key {
            "follow_cell_length" => self.set_follow_cell_length(scheduled_bool(value)?),
            "wait_for_cycle_end" => self.set_wait_for_cycle_end(scheduled_bool(value)?),
            _ => {
                let value = scheduled_number(value)?;
                match key {
                    "root_note" => self.set_root_note(value as u8),
                    "step_count" => self.set_step_count(value as usize),
                    "gate_length" => self.set_gate_length(value),
                    "select_cell" => self.set_selected_cell(value.max(0.0) as usize),
                    "grace_duration" => self.set_grace_duration(value),
                    "grace_velocity" => self.set_grace_velocity(value),
                    "next_cell" if value > 0.5 => self.request_next_cell(),
                    "next_cell" => {}
                    _ => return Err(LegacyRefusal::Unsupported),
                }
            }
        }
        Ok(())
    }

    /// What `get_control` reads of a number, the bank's length included,
    /// without its lock.
    pub(super) fn scheduled_read(&self, key: &str) -> Result<f32, LegacyRefusal> {
        Ok(match key {
            "root_note" => self.root_note() as f32,
            "step_count" => self.step_count() as f32,
            "gate_length" => self.gate_length(),
            "select_cell" => self.selected_cell() as f32,
            "grace_duration" => self.grace_duration(),
            "grace_velocity" => self.grace_velocity(),
            "loop_count" => self.loop_count() as f32,
            "cell" => self.cell() as f32,
            "cell_count" => self.bank_len() as f32,
            "next_cell" => 0.0,
            _ => return Err(LegacyRefusal::Unsupported),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alloc_counter::allocator_events;
    use crate::control_request::while_held;
    use crate::ControlSurface;

    #[test]
    fn a_scheduled_cell_selection_lands_while_the_bank_is_held() {
        let controls = CellSequencerControls::new();
        controls.set_cells_json("[[60], [62], [64]]").unwrap();
        let (written, allocs, frees) = while_held(
            || controls.shared.sequences.lock().unwrap(),
            || {
                allocator_events(|| {
                    let selected = controls.set_scheduled("select_cell", TableValue::Number(9.0));
                    (selected, controls.get_scheduled("cell_count"))
                })
            },
        );
        assert_eq!((allocs, frees), (0, 0));
        assert_eq!(written, (Ok(()), Ok(3.0)));
        assert_eq!(controls.selected_cell(), 2, "clamped to the bank");
        controls.set_cells_json("[[60]]").unwrap();
        assert_eq!(controls.get_scheduled("cell_count"), Ok(1.0));
        assert_eq!(
            controls.set_scheduled("select_cell", TableValue::Number(1.0)),
            Ok(())
        );
        assert_eq!(controls.selected_cell(), 0);
        let read_only = controls.set_scheduled("cell", TableValue::Number(1.0));
        assert_eq!(read_only, Err(LegacyRefusal::Unsupported));
    }

    /// Races a scheduled `select_cell` of 2 against replacing a bank of
    /// three with `cells`, `rounds` times, returning each round's selection.
    fn race_selection_with(cells: &'static str, rounds: usize) -> Vec<usize> {
        let barrier = std::sync::Barrier::new(2);
        (0..rounds)
            .map(|_| {
                let controls = CellSequencerControls::new();
                controls.set_cells_json("[[60], [62], [64]]").unwrap();
                std::thread::scope(|scope| {
                    scope.spawn(|| {
                        barrier.wait();
                        controls.set_cells_json(cells).unwrap();
                    });
                    barrier.wait();
                    let selected = controls.set_scheduled("select_cell", TableValue::Number(2.0));
                    assert_eq!(selected, Ok(()));
                });
                controls.selected_cell()
            })
            .collect()
    }

    #[test]
    fn a_scheduled_selection_racing_a_bank_replacement_lands_before_or_after_it() {
        // Either order selects cell 2 of a bank of three.
        let selected = race_selection_with("[[61], [63], [65]]", 2_000);
        assert!(selected.iter().all(|&cell| cell == 2), "{selected:?}");
        // Either order leaves a bank of one with its only cell selected.
        let selected = race_selection_with("[[61]]", 2_000);
        assert!(selected.iter().all(|&cell| cell == 0), "{selected:?}");
    }
}
