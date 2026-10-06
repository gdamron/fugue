//! Single-position and count controls, which a `control_scheduler` reads
//! and writes from the audio thread: none of them ever blocks (see
//! [`super::pending`]).

use std::sync::atomic::Ordering;

use super::controls::{out_of_range, MelodyControls, MAX_DEGREES};

impl MelodyControls {
    /// Gets the number of active degrees. Never blocks.
    pub fn degree_count(&self) -> usize {
        match self.try_lock_table() {
            Some(table) => table.count(),
            None => self.pending.latest_count(),
        }
    }

    /// Sets the number of active degrees (1 to [`MAX_DEGREES`]). Never
    /// blocks: a write that finds the table busy is deferred.
    ///
    /// Shrinking hides the positions past the count; growing shows them again
    /// as they were. Positions past the base scale repeat it from the start.
    pub fn set_degree_count(&self, count: usize) {
        let count = count.clamp(1, MAX_DEGREES);
        match self.try_lock_table() {
            // A ramp writes the count every sample; only a change is an edit.
            Some(mut table) => {
                if table.set_count(count) {
                    self.pending.publish(&table);
                    self.table_version.fetch_add(1, Ordering::Release);
                }
            }
            None => {
                if self.pending.latest_count() != count {
                    self.pending.deposit_count(count);
                    self.retry_drain();
                }
            }
        }
    }

    /// Gets the scale degree at active position `index`. Never blocks.
    pub fn degree(&self, index: usize) -> Result<i32, String> {
        match self.try_lock_table() {
            Some(table) => table
                .degrees
                .get(index)
                .copied()
                .ok_or_else(|| out_of_range("Degree", index, table.count())),
            None => self.pending.degree(index),
        }
    }

    /// Sets the scale degree at active position `index` (clamped to ±127).
    /// Never blocks: a write that finds the table busy is deferred.
    pub fn set_degree(&self, index: usize, value: i32) -> Result<(), String> {
        let value = value.clamp(-127, 127);
        let Some(mut table) = self.try_lock_table() else {
            // Load the generation before validating (see the `pending` docs).
            let generation = self.pending.degrees_gen();
            self.pending.check_index("Degree", index)?;
            self.pending.deposit_degree(generation, index, value);
            self.retry_drain();
            return Ok(());
        };
        if index >= table.count() {
            return Err(out_of_range("Degree", index, table.count()));
        }
        table.write_degree(index, value);
        self.pending.publish_degree(index, value);
        self.table_version.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Gets the note weight at active position `index`. Never blocks.
    pub fn note_weight(&self, index: usize) -> Result<f32, String> {
        match self.try_lock_table() {
            Some(table) => table
                .weights
                .get(index)
                .copied()
                .ok_or_else(|| out_of_range("Weight", index, table.count())),
            None => self.pending.weight(index),
        }
    }

    /// Sets the note weight at active position `index` (clamped to 0-10).
    /// Never blocks: a write that finds the table busy is deferred.
    pub fn set_note_weight(&self, index: usize, value: f32) -> Result<(), String> {
        let value = value.clamp(0.0, 10.0);
        let Some(mut table) = self.try_lock_table() else {
            // Load the generation before validating (see the `pending` docs).
            let generation = self.pending.weights_gen();
            self.pending.check_index("Weight", index)?;
            self.pending.deposit_weight(generation, index, value);
            self.retry_drain();
            return Ok(());
        };
        if index >= table.count() {
            return Err(out_of_range("Weight", index, table.count()));
        }
        table.write_weight(index, value);
        self.pending.publish_weight(index, value);
        self.table_version.fetch_add(1, Ordering::Release);
        Ok(())
    }
}
