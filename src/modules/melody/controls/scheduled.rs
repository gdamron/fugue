//! A melody's legacy controls as a scheduler writes them, on the audio
//! thread (see [`crate::ControlSurface::set_scheduled`]): every one is a
//! store into its atomic, the degree table's included (see
//! [`super::DegreeTable`]), so none waits and none is refused for a lock.

use super::MelodyControls;
use crate::control_request::{scheduled_index, scheduled_number, LegacyRefusal, TableValue};

impl MelodyControls {
    /// What `set_control` writes, formatting no error.
    pub(super) fn scheduled_write(
        &self,
        key: &str,
        value: TableValue,
    ) -> Result<(), LegacyRefusal> {
        let value = scheduled_number(value)?;
        let written = match key {
            "root_note" => {
                self.set_root_note(value as u8);
                true
            }
            "seed" => {
                self.set_seed(value.max(0.0) as u64);
                true
            }
            "degree_count" => {
                self.table.set_count(value as usize);
                true
            }
            _ => {
                if let Some(index) = scheduled_index(key, "degree.") {
                    self.table.write_degree(index, value as i32)
                } else if let Some(index) = scheduled_index(key, "note_weight.") {
                    self.table.write_weight(index, value)
                } else {
                    return Err(LegacyRefusal::Unsupported);
                }
            }
        };
        written.then_some(()).ok_or(LegacyRefusal::Invalid)
    }

    /// What `get_control` reads, formatting no error.
    pub(super) fn scheduled_read(&self, key: &str) -> Result<f32, LegacyRefusal> {
        let table = &self.table;
        match key {
            "root_note" => Ok(self.root_note() as f32),
            "seed" => Ok(self.seed().unwrap_or(0) as f32),
            "degree_count" => Ok(table.count() as f32),
            _ => {
                let (index, degree) = if let Some(index) = scheduled_index(key, "degree.") {
                    (index, true)
                } else if let Some(index) = scheduled_index(key, "note_weight.") {
                    (index, false)
                } else {
                    return Err(LegacyRefusal::Unsupported);
                };
                let (at, weight) = table.position(index).ok_or(LegacyRefusal::Invalid)?;
                Ok(if degree { at as f32 } else { weight })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alloc_counter::allocator_events;
    use crate::audio_thread::AudioThreadScope;
    use crate::ControlSurface;

    #[test]
    fn a_scheduled_write_to_the_degree_table_lands_mid_edit_without_a_lock() {
        let controls = MelodyControls::new(60, vec![0, 2, 4]);
        let version = controls.table_version();
        let number = TableValue::Number(1.0);
        let table_keys = ["degree_count", "degree.0", "note_weight.0"];
        // A control thread mid-way through an edit of the whole table.
        let editing = controls.table.editing();
        let (landed, allocs, frees) = allocator_events(|| {
            let _audio = AudioThreadScope::enter();
            let writes = table_keys.map(|key| controls.set_scheduled(key, number));
            let reads = table_keys.map(|key| controls.get_scheduled(key));
            (writes, reads)
        });
        drop(editing);
        assert_eq!((allocs, frees), (0, 0));
        assert_eq!(landed.0, [Ok(()); 3]);
        assert_eq!(landed.1, [Ok(1.0), Ok(1.0), Ok(1.0)]);
        assert!(controls.table_version() > version);
        assert_eq!(controls.degree(0), Ok(1));
        assert_eq!(
            controls.set_scheduled("degree.7", number),
            Err(LegacyRefusal::Invalid),
            "a position past the count"
        );
    }

    #[test]
    fn scheduled_writes_and_the_notes_they_shape_take_no_control_lock() {
        use crate::modules::melody::MelodyGenerator;
        use crate::Module;
        let controls = MelodyControls::new(60, vec![0, 2, 4]);
        let mut melody = MelodyGenerator::new(controls.clone());
        let keys = [
            "degree_count",
            "degree.4",
            "note_weight.4",
            "root_note",
            "seed",
        ];
        let values = [5.0, 7.0, 10.0, 62.0, 3.0];
        // On the audio thread, a control-only lock (the table's, for an edit
        // of many entries) would panic in a debug build.
        let (frequency, allocs, frees) = allocator_events(|| {
            let _audio = AudioThreadScope::enter();
            for (key, value) in keys.into_iter().zip(values) {
                assert_eq!(
                    controls.set_scheduled(key, TableValue::Number(value)),
                    Ok(())
                );
            }
            for gate in [1.0, 0.0] {
                melody.set_input("gate", gate).unwrap();
                melody.process(1);
            }
            melody.get_output("frequency").unwrap()
        });
        assert_eq!((allocs, frees), (0, 0));
        assert!(frequency > 0.0);
        assert_eq!(controls.allowed_degrees(), [0, 2, 4, 0, 7]);
        assert_eq!(controls.note_weight(4), Ok(10.0));
    }
}
