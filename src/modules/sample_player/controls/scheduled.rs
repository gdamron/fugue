//! A sample player's legacy controls as a scheduler writes them, on the
//! audio thread (see [`crate::ControlSurface::set_scheduled`]): every one a
//! scheduler can reach is an atomic, so each write is a store.

use super::SamplePlayerControls;
use crate::control_request::{scheduled_bool, scheduled_number, LegacyRefusal, TableValue};

impl SamplePlayerControls {
    /// What `set_control` writes, refused rather than formatting an
    /// error. `asset` is a string, which no schedule writes.
    pub(super) fn scheduled_write(
        &self,
        key: &str,
        value: TableValue,
    ) -> Result<(), LegacyRefusal> {
        match key {
            "play" => self.set_play(scheduled_bool(value)?),
            "loop" => self.set_loop_enabled(scheduled_bool(value)?),
            "pitch_ratio" => self.set_pitch_ratio(scheduled_number(value)?),
            "time_ratio" if self.elastic => self.set_time_ratio(scheduled_number(value)?),
            _ => return Err(LegacyRefusal::Unsupported),
        }
        Ok(())
    }

    /// What `get_control` reads of a number.
    pub(super) fn scheduled_read(&self, key: &str) -> Result<f32, LegacyRefusal> {
        match key {
            "pitch_ratio" => Ok(self.pitch_ratio()),
            "time_ratio" if self.elastic => Ok(self.time_ratio()),
            _ => Err(LegacyRefusal::Unsupported),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::transport_trigger;
    use super::*;
    use crate::alloc_counter::allocator_events;
    use crate::audio_thread::AudioThreadScope;
    use crate::control_request::while_held;
    use crate::ControlSurface;

    #[test]
    fn scheduled_playback_writes_land_while_a_control_thread_swaps_the_sample() {
        let controls = SamplePlayerControls::with_mode(48_000, None, None, None, true).unwrap();
        let on = TableValue::Bool(true);
        let trigger = transport_trigger(controls.transport());
        let (written, allocs, frees) = while_held(
            || controls.hold_swap_lock(),
            || {
                let _audio = AudioThreadScope::enter();
                allocator_events(|| {
                    [
                        controls.set_scheduled("play", on),
                        controls.set_scheduled("play", on),
                        controls.set_scheduled("loop", on),
                        controls.set_scheduled("time_ratio", TableValue::Number(2.0)),
                        controls.set_scheduled("pitch_ratio", TableValue::Number(0.5)),
                    ]
                })
            },
        );
        assert_eq!((allocs, frees), (0, 0));
        assert_eq!(written, [Ok(()); 5]);
        assert!(controls.play() && controls.loop_enabled());
        let retriggered = transport_trigger(controls.transport());
        assert_eq!(retriggered, trigger + 2, "each start retriggers");
        assert_eq!((controls.time_ratio(), controls.pitch_ratio()), (2.0, 0.5));
        let refused = [
            controls.set_scheduled("play", TableValue::Number(1.0)),
            controls.set_scheduled("asset", TableValue::Number(1.0)),
        ];
        assert_eq!(refused, [Err(LegacyRefusal::Unsupported); 2]);
    }
}
