//! An instrument's legacy controls as a scheduler writes them, on the
//! audio thread (see [`crate::ControlSurface::set_scheduled`]).
//!
//! A note is pushed onto the schedulers' own lock-free note queue: refused
//! only when that is full, and the scheduler counts it.

use super::{indexed_key, midi_note, release_ok, SampleInstrumentControls};
use crate::control_request::{scheduled_number, LegacyRefusal, TableValue};

impl SampleInstrumentControls {
    /// What `set_control` writes, refused rather than formatting an
    /// error.
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
                (self.inner.notes.schedule(note, key == "note_on"))
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
    use super::super::{ZoneSpec, LANES, NOTE_CAPACITY};
    use super::*;
    use crate::alloc_counter::allocator_events;
    use crate::audio_thread::AudioThreadScope;
    use crate::module_config::tests::registry::eight_frame_wav;
    use crate::modules::sample_instrument::SampleInstrument;
    use crate::Module;
    use crate::{ControlSurface, ControlValue};

    fn zone() -> ZoneSpec {
        ZoneSpec {
            root_note: 60,
            key_low: 0,
            key_high: 127,
            asset: eight_frame_wav().to_string(),
            level: 1.0,
            loop_spec: None,
        }
    }

    fn instrument() -> SampleInstrumentControls {
        SampleInstrumentControls::new(48_000, 0.1, vec![zone()])
            .unwrap()
            .0
    }

    /// An instrument with `voices` voices, and which notes it is sounding.
    fn sounding_with(
        voices: usize,
    ) -> (
        SampleInstrumentControls,
        SampleInstrument,
        impl Fn(&SampleInstrument) -> Vec<u8>,
    ) {
        let (controls, audio, queues) =
            SampleInstrumentControls::new(48_000, 0.1, vec![zone()]).unwrap();
        let instrument =
            SampleInstrument::new_with_controls(controls.clone(), audio, queues, voices, 48_000);
        let sounding = |instrument: &SampleInstrument| -> Vec<u8> {
            let voices = instrument.voices.iter().filter(|voice| voice.active);
            let mut notes: Vec<u8> = voices.map(|voice| voice.note).collect();
            notes.sort_unstable();
            notes
        };
        (controls, instrument, sounding)
    }

    #[test]
    fn a_control_thread_held_up_mid_push_holds_up_no_other_note() {
        let (controls, mut instrument, sounding) = sounding_with(1);
        // One voice. A control thread is preempted mid-push; another sends
        // a note on meanwhile, then a scheduler's note on plays: in the
        // order sent, 60 sounds and 64 steals its voice.
        let publish = controls.held_up_note();
        controls.note_on(&ControlValue::Number(60.0)).unwrap();
        let scheduled = controls.set_scheduled("note_on", TableValue::Number(64.0));
        assert_eq!(scheduled, Ok(()));
        instrument.process(1);
        assert_eq!(sounding(&instrument), [64]);
        // The held-up push lands a block later, and takes the voice.
        publish(72, true);
        instrument.process(1);
        assert_eq!(sounding(&instrument), [72]);
        controls.note_on(&ControlValue::Number(67.0)).unwrap();
        instrument.process(1);
        assert_eq!(sounding(&instrument), [67]);
    }

    #[test]
    fn notes_sent_past_a_held_up_push_take_voices_in_the_order_sent() {
        let (controls, mut instrument, sounding) = sounding_with(2);
        // Two voices: 60, then 64, then 67 steals the oldest, 60.
        let publish = controls.held_up_note();
        controls.note_on(&ControlValue::Number(60.0)).unwrap();
        let scheduled = controls.set_scheduled("note_on", TableValue::Number(64.0));
        assert_eq!(scheduled, Ok(()));
        instrument.process(1);
        assert_eq!(sounding(&instrument), [60, 64]);
        publish(72, false);
        instrument.process(1);
        controls.note_on(&ControlValue::Number(67.0)).unwrap();
        instrument.process(1);
        assert_eq!(sounding(&instrument), [64, 67]);
    }

    #[test]
    fn a_scheduled_note_finds_room_however_many_control_threads_send() {
        let controls = instrument();
        let note = ControlValue::Number(64.0);
        let sent = std::iter::repeat_with(|| controls.note_on(&note).is_ok());
        let queued = sent.take_while(|&queued| queued).count();
        assert_eq!(queued, LANES * NOTE_CAPACITY);
        let scheduled = TableValue::Number(67.0);
        let _audio = AudioThreadScope::enter();
        let ((), allocs, frees) = allocator_events(|| {
            for _ in 0..NOTE_CAPACITY {
                assert_eq!(controls.set_scheduled("note_on", scheduled), Ok(()));
            }
            let full = controls.set_scheduled("note_off", scheduled);
            assert_eq!(full, Err(LegacyRefusal::Full));
            let invalid = controls.set_scheduled("note_off", TableValue::Number(200.0));
            assert_eq!(invalid, Err(LegacyRefusal::Invalid));
            let release = controls.set_scheduled("release", TableValue::Number(-1.0));
            assert_eq!(release, Err(LegacyRefusal::Invalid));
            let level = controls.set_scheduled("level.0", TableValue::Number(0.5));
            assert_eq!(level, Ok(()));
        });
        assert_eq!((allocs, frees), (0, 0));
    }
}
