//! A melody's legacy controls as a scheduler writes them, on the audio
//! thread (see [`crate::ControlSurface::set_scheduled`]).
//!
//! The degree table's controls take its lock without waiting: when a
//! control thread holds it (an edit, or `controls()` listing the table),
//! the write is refused as busy, and the scheduler counts it. They are read
//! from the table's mirror, never refused, so a ramp always has its start.

use std::sync::MutexGuard;

use super::{DegreeTable, MelodyControls};
use crate::control_request::{
    scheduled_index, scheduled_number, try_scheduled, LegacyRefusal, TableValue,
};

impl MelodyControls {
    /// The degree table, if no other thread holds it.
    fn try_table(&self) -> Result<MutexGuard<'_, DegreeTable>, LegacyRefusal> {
        try_scheduled(&self.table)
    }

    /// What `set_control` writes, refused rather than waiting on the table.
    pub(super) fn scheduled_write(
        &self,
        key: &str,
        value: TableValue,
    ) -> Result<(), LegacyRefusal> {
        let value = scheduled_number(value)?;
        match key {
            "root_note" => self.set_root_note(value as u8),
            "seed" => self.set_seed(value.max(0.0) as u64),
            "degree_count" => {
                let mut table = self.try_table()?;
                if table.set_count(value as usize) {
                    self.publish(&table);
                }
            }
            _ => {
                let mut table = self.try_table()?;
                let written = if let Some(index) = scheduled_index(key, "degree.") {
                    table.write_degree(index, value as i32)
                } else if let Some(index) = scheduled_index(key, "note_weight.") {
                    table.write_weight(index, value)
                } else {
                    return Err(LegacyRefusal::Unsupported);
                };
                if !written {
                    return Err(LegacyRefusal::Invalid);
                }
                self.publish(&table);
            }
        }
        Ok(())
    }

    /// What `get_control` reads, from the mirror rather than the table.
    pub(super) fn scheduled_read(&self, key: &str) -> Result<f32, LegacyRefusal> {
        match key {
            "root_note" => Ok(self.root_note() as f32),
            "seed" => Ok(self.seed().unwrap_or(0) as f32),
            "degree_count" => Ok(self.mirror.count() as f32),
            _ => {
                let read = if let Some(index) = scheduled_index(key, "degree.") {
                    self.mirror.degree(index).map(|degree| degree as f32)
                } else if let Some(index) = scheduled_index(key, "note_weight.") {
                    self.mirror.weight(index)
                } else {
                    return Err(LegacyRefusal::Unsupported);
                };
                read.ok_or(LegacyRefusal::Invalid)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alloc_counter::allocator_events;
    use crate::control_request::while_held;
    use crate::ControlSurface;

    #[test]
    fn a_scheduled_write_to_a_held_degree_table_is_refused_without_waiting() {
        let controls = MelodyControls::new(60, vec![0, 2, 4]);
        let version = controls.table_version();
        let number = TableValue::Number(1.0);
        let table_keys = ["degree_count", "degree.0", "note_weight.0"];
        let (refused, allocs, frees) = while_held(&controls.table, || {
            allocator_events(|| {
                let writes = table_keys.map(|key| controls.set_scheduled(key, number));
                let reads = table_keys.map(|key| controls.get_scheduled(key));
                let root = controls.set_scheduled("root_note", TableValue::Number(62.0));
                (writes, reads, root)
            })
        });
        assert_eq!((allocs, frees), (0, 0));
        assert_eq!(refused.0, [Err(LegacyRefusal::Busy); 3]);
        assert_eq!(
            refused.1,
            [Ok(3.0), Ok(0.0), Ok(1.0)],
            "read from the mirror"
        );
        assert_eq!(refused.2, Ok(()), "an atomic control needs no lock");
        assert_eq!(controls.root_note(), 62);
        assert_eq!(controls.table_version(), version, "nothing published");
        // Released, the table takes the write.
        assert_eq!(
            controls.set_scheduled("degree.0", TableValue::Number(5.0)),
            Ok(())
        );
        assert_eq!(controls.degree(0), Ok(5));
        assert!(controls.table_version() > version);
        assert_eq!(
            controls.set_scheduled("degree.7", number),
            Err(LegacyRefusal::Invalid),
            "a position past the count"
        );
    }
}
