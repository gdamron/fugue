//! How the cell sequencer applies its declared controls and its bank, on
//! the thread that runs it: allocation-, free- and lock-free.

use super::*;

impl CellSequencer {
    pub(super) fn apply_control(
        &mut self,
        control: ControlIndex,
        value: RtValue,
    ) -> Result<RtValue, Refusal> {
        Ok(match (control, value) {
            (ROOT_NOTE, RtValue::I32(note)) => {
                self.root_note = note.clamp(0, 127) as u8;
                RtValue::I32(i32::from(self.root_note))
            }
            (STEP_COUNT, RtValue::I32(count)) => {
                self.step_count = count.clamp(1, MAX_STEPS as i32) as usize;
                RtValue::I32(self.step_count as i32)
            }
            (FOLLOW_CELL_LENGTH, RtValue::Bool(follow)) => {
                self.follow_cell_length = follow;
                value
            }
            (GATE_LENGTH, RtValue::F32(length)) => {
                self.gate_length = length.clamp(0.0, 1.0);
                RtValue::F32(self.gate_length)
            }
            // A request, acted on at the next sample and clamped into the
            // bank then (a bank written with it may land after it); it reads
            // back as the cell asked for.
            (SELECT_CELL, RtValue::I32(cell)) => {
                let cell = cell.clamp(0, MAX_SEQUENCES as i32 - 1);
                self.requested = Some(cell as usize);
                RtValue::I32(cell)
            }
            (WAIT_FOR_CYCLE_END, RtValue::Bool(wait)) => {
                self.wait_for_cycle_end = wait;
                value
            }
            (MODE, RtValue::U32(mode @ 0..=1)) => {
                self.one_shot = mode == 1;
                value
            }
            (NEXT_CELL, RtValue::F32(trigger)) => {
                if trigger > 0.5 {
                    self.next_requested = self.next_requested.saturating_add(1);
                }
                RtValue::F32(0.0)
            }
            (GRACE_DURATION, RtValue::F32(seconds)) => {
                self.grace_duration = seconds.clamp(MIN_GRACE_DURATION, MAX_GRACE_DURATION);
                RtValue::F32(self.grace_duration)
            }
            (GRACE_VELOCITY, RtValue::F32(scale)) => {
                self.grace_velocity = scale.clamp(0.0, 1.0);
                RtValue::F32(self.grace_velocity)
            }
            (GRACE_PLACEMENT, RtValue::U32(placement @ 0..=1)) => {
                self.grace_on_beat = placement == 1;
                value
            }
            (MODE | GRACE_PLACEMENT, RtValue::U32(_)) => return Err(Refusal::Invalid),
            _ => return Err(Refusal::Unsupported),
        })
    }

    /// Takes a written bank from the next sample: the step playing keeps
    /// sounding, and the next clock edge reads the new bank (a rest past a
    /// cell's end, as ever).
    pub(super) fn apply_bank(
        &mut self,
        control: ControlIndex,
        payload: Payload,
    ) -> Result<Retired, (Refusal, Payload)> {
        if control != CELLS {
            return Err((Refusal::Unsupported, payload));
        }
        match payload.downcast::<Bank>() {
            Ok(bank) => Ok(self.adopt_bank(bank).into()),
            Err(payload) => Err((Refusal::Invalid, payload)),
        }
    }
}
