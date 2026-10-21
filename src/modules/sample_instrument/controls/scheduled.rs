//! An instrument's legacy controls as a scheduler writes them, on the
//! audio thread (see [`crate::ControlSurface::set_scheduled`]).
//!
//! A note takes the pending queue's lock without waiting: when a control
//! thread holds it (queueing a note of its own), the note is refused as
//! busy, and the scheduler counts it, as it does one finding the queue
//! full.

use super::{indexed_key, midi_note, release_ok, SampleInstrumentControls};
use crate::control_request::{scheduled_number, try_scheduled, LegacyRefusal, TableValue};

impl SampleInstrumentControls {
    /// What `set_control` writes, refused rather than waiting on the note
    /// queue or formatting an error.
    pub(super) fn scheduled_write(
        &self,
        key: &str,
        value: TableValue,
    ) -> Result<(), LegacyRefusal> {
        let value = scheduled_number(value)?;
        match key {
            "release" if !release_ok(value) => Err(LegacyRefusal::Invalid),
            "release" => {
                self.inner.release.store(value.clamp(1e-3, 30.0));
                Ok(())
            }
            "note_on" | "note_off" => {
                let note = midi_note(value).ok_or(LegacyRefusal::Invalid)?;
                let pending = try_scheduled(&self.inner.pending_notes)?;
                (self.queue_note(pending, note, key == "note_on"))
                    .then_some(())
                    .ok_or(LegacyRefusal::Full)
            }
            _ => {
                let zone = (indexed_key(key, "level."))
                    .and_then(|index| self.inner.zones.get(index))
                    .ok_or(LegacyRefusal::Unsupported)?;
                zone.level.store(value.clamp(0.0, 2.0));
                Ok(())
            }
        }
    }

    /// What `get_control` reads of a number: a note control reads 0, as
    /// it holds no note of its own.
    pub(super) fn scheduled_read(&self, key: &str) -> Result<f32, LegacyRefusal> {
        match key {
            "release" => Ok(self.release()),
            "note_on" | "note_off" => Ok(0.0),
            _ => {
                let zone = |prefix| {
                    (indexed_key(key, prefix)).and_then(|index| self.inner.zones.get(index))
                };
                if let Some(zone) = zone("root_note.") {
                    Ok(zone.root_note as f32)
                } else if let Some(zone) = zone("level.") {
                    Ok(zone.level.load())
                } else {
                    Err(LegacyRefusal::Unsupported)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{ZoneSpec, MAX_PENDING_NOTES};
    use super::*;
    use crate::alloc_counter::allocator_events;
    use crate::control_request::while_held;
    use crate::module_config::tests::registry::eight_frame_wav;
    use crate::ControlSurface;

    fn instrument() -> SampleInstrumentControls {
        let zone = ZoneSpec {
            root_note: 60,
            key_low: 0,
            key_high: 127,
            asset: eight_frame_wav().to_string(),
            level: 1.0,
            loop_spec: None,
        };
        SampleInstrumentControls::new(48_000, 0.1, vec![zone])
            .unwrap()
            .0
    }

    #[test]
    fn a_scheduled_note_finding_the_queue_held_or_full_is_refused_without_waiting() {
        let controls = instrument();
        let version = controls.notes_version();
        let note = TableValue::Number(64.0);
        let (written, allocs, frees) = while_held(&controls.inner.pending_notes, || {
            allocator_events(|| {
                let notes = ["note_on", "note_off"].map(|key| controls.set_scheduled(key, note));
                (
                    notes,
                    controls.set_scheduled("level.0", TableValue::Number(0.5)),
                )
            })
        });
        assert_eq!((allocs, frees), (0, 0));
        assert_eq!(written.0, [Err(LegacyRefusal::Busy); 2]);
        assert_eq!(written.1, Ok(()), "an atomic control needs no lock");
        assert_eq!(controls.notes_version(), version, "nothing queued");
        // Released, the queue takes notes until it is full.
        let ((), allocs, frees) = allocator_events(|| {
            for _ in 0..MAX_PENDING_NOTES {
                assert_eq!(controls.set_scheduled("note_on", note), Ok(()));
            }
            let full = controls.set_scheduled("note_on", note);
            assert_eq!(full, Err(LegacyRefusal::Full));
            let invalid = controls.set_scheduled("note_off", TableValue::Number(200.0));
            assert_eq!(invalid, Err(LegacyRefusal::Invalid));
            let release = controls.set_scheduled("release", TableValue::Number(-1.0));
            assert_eq!(release, Err(LegacyRefusal::Invalid));
        });
        assert_eq!((allocs, frees), (0, 0));
        assert_eq!(controls.notes_version(), version + MAX_PENDING_NOTES as u64);
    }
}
