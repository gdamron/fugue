//! Thread-safe controls for the MelodyGenerator module.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use crate::control_request::{LegacyRefusal, TableValue};
use crate::{ControlMeta, ControlSurface, ControlValue};

mod scheduled;
mod table;

pub(crate) use table::{DegreeTable, MAX_DEGREES};

/// Thread-safe controls for the MelodyGenerator module.
///
/// Every field is atomic, the degree table (degrees + weights) included (see
/// [`DegreeTable`]): the audio thread keeps its own pre-allocated copy and
/// re-copies it when its version changes, never waiting on a control
/// thread, and a scheduler writes it with a store.
///
/// Note: Due to the complex types (Vec), this module exposes typed methods
/// rather than the uniform f32 get/set_control API for most parameters.
///
/// # Example
///
/// ```rust,ignore
/// let controls: MelodyControls = handles.get("melody.controls").unwrap();
///
/// // Adjust melody parameters in real-time
/// controls.set_allowed_degrees(vec![0, 2, 4, 5, 7]); // Pentatonic subset
/// controls.set_note_weights(vec![1.0, 0.5, 0.8, 0.3, 1.0]);
/// ```
#[derive(Clone)]
pub struct MelodyControls {
    /// Root MIDI note number (0-127).
    pub(crate) root_note: Arc<AtomicU8>,
    /// Allowed scale degrees and their weights.
    pub(crate) table: Arc<DegreeTable>,
    /// RNG seed value; only meaningful when `seed_version > 0`.
    pub(crate) seed_value: Arc<AtomicU64>,
    /// Bumped on every `set_seed`; `0` means "never seeded" (entropy RNG).
    /// The audio thread re-seeds when it observes a version change, so
    /// setting the same seed again deterministically restarts the stream.
    pub(crate) seed_version: Arc<AtomicU64>,
}

impl MelodyControls {
    /// Creates new melody controls with the given allowed scale degrees.
    ///
    /// All degrees start with equal probability weight. At most
    /// [`MAX_DEGREES`] degrees are kept.
    pub fn new(root_note: u8, mut allowed_degrees: Vec<i32>) -> Self {
        allowed_degrees.truncate(MAX_DEGREES);
        Self {
            root_note: Arc::new(AtomicU8::new(root_note)),
            table: Arc::new(DegreeTable::new(allowed_degrees)),
            seed_value: Arc::new(AtomicU64::new(0)),
            seed_version: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Returns the RNG seed, or `None` when the generator runs from entropy.
    pub fn seed(&self) -> Option<u64> {
        if self.seed_version.load(Ordering::Acquire) == 0 {
            None
        } else {
            Some(self.seed_value.load(Ordering::Acquire))
        }
    }

    /// Seeds the RNG. Identical seeds produce identical note sequences;
    /// setting the same seed again restarts the stream from the top.
    pub fn set_seed(&self, seed: u64) {
        self.seed_value.store(seed, Ordering::Release);
        self.seed_version.fetch_add(1, Ordering::Release);
    }

    /// Monotonic seed change counter (0 = never seeded).
    pub fn seed_version(&self) -> u64 {
        self.seed_version.load(Ordering::Acquire)
    }

    /// Gets the root MIDI note number.
    pub fn root_note(&self) -> u8 {
        self.root_note.load(Ordering::Relaxed)
    }

    /// Sets the root MIDI note number (clamped to 0-127).
    pub fn set_root_note(&self, value: u8) {
        self.root_note.store(value.min(127), Ordering::Relaxed);
    }

    /// Monotonic degree-table change counter.
    pub(crate) fn table_version(&self) -> u64 {
        self.table.version()
    }

    /// Gets the active scale degrees.
    pub fn allowed_degrees(&self) -> Vec<i32> {
        self.table.degrees()
    }

    /// Sets the base scale: which degrees can be used for note selection.
    ///
    /// The count becomes its length, and degrees written to single positions
    /// are forgotten; weights are kept. At most [`MAX_DEGREES`] degrees are
    /// kept.
    pub fn set_allowed_degrees(&self, degrees: Vec<i32>) {
        self.table.set_scale(degrees);
    }

    /// Gets the active note weights.
    pub fn note_weights(&self) -> Vec<f32> {
        self.table.weights()
    }

    /// Sets the base probability weights for note selection, forgetting
    /// weights written to single positions.
    ///
    /// Higher weights make that degree more likely to be chosen. A position
    /// without a weight of its own takes the weight of the scale position it
    /// repeats, or 1.0.
    pub fn set_note_weights(&self, weights: Vec<f32>) {
        self.table.set_scale_weights(weights);
    }

    /// Gets the number of active degrees.
    pub fn degree_count(&self) -> usize {
        self.table.count()
    }

    /// Sets the number of active degrees (1 to [`MAX_DEGREES`]).
    ///
    /// Shrinking hides the positions past the count; growing shows them again
    /// as they were. Positions past the base scale repeat it from the start.
    pub fn set_degree_count(&self, count: usize) {
        // A ramp writes the count every sample; only a change is an edit.
        self.table.set_count(count);
    }

    /// Gets the scale degree at active position `index`. Waits only if whole
    /// edits outrun every retry of a lock-free read.
    pub fn degree(&self, index: usize) -> Result<i32, String> {
        (self.table.position_or_wait(index))
            .map(|(degree, _)| degree)
            .ok_or_else(|| self.out_of_range("Degree", index))
    }

    /// Sets the scale degree at active position `index` (clamped to ±127).
    pub fn set_degree(&self, index: usize, value: i32) -> Result<(), String> {
        (self.table.write_degree(index, value))
            .then_some(())
            .ok_or_else(|| self.out_of_range("Degree", index))
    }

    /// Gets the note weight at active position `index`. Waits only if whole
    /// edits outrun every retry of a lock-free read.
    pub fn note_weight(&self, index: usize) -> Result<f32, String> {
        (self.table.position_or_wait(index))
            .map(|(_, weight)| weight)
            .ok_or_else(|| self.out_of_range("Weight", index))
    }

    /// Sets the note weight at active position `index` (clamped to 0-10).
    pub fn set_note_weight(&self, index: usize, value: f32) -> Result<(), String> {
        (self.table.write_weight(index, value))
            .then_some(())
            .ok_or_else(|| self.out_of_range("Weight", index))
    }

    fn out_of_range(&self, what: &str, index: usize) -> String {
        let count = self.table.count();
        format!("{what} index {index} out of range (count: {count})")
    }

    /// Restores a degree and a weight written to single positions, as a
    /// document records them, whether or not the position is active now: a
    /// hidden one shows again when the count grows. For building only.
    pub(crate) fn restore_written(
        &self,
        degrees: impl IntoIterator<Item = (usize, i32)>,
        weights: impl IntoIterator<Item = (usize, f32)>,
    ) {
        self.table.restore_written(degrees, weights);
    }
}

impl ControlSurface for MelodyControls {
    fn controls(&self) -> Vec<ControlMeta> {
        let degree_count = self.degree_count();
        let mut controls = Vec::with_capacity(3 + degree_count * 2);
        controls.push(
            ControlMeta::number("root_note", "Root MIDI note number")
                .with_range(0.0, 127.0)
                .with_default(self.root_note() as f32),
        );
        controls.push(
            ControlMeta::number("degree_count", "Number of active scale degrees")
                .with_range(1.0, 128.0)
                .with_default(degree_count as f32),
        );
        controls.push(
            ControlMeta::number(
                "seed",
                "RNG seed: same seed, same melody; setting it (re)starts the \
                 stream. Note: f32 controls carry ~24 bits of integer \
                 precision; use module config JSON for full 64-bit seeds.",
            )
            .with_default(self.seed().unwrap_or(0) as f32),
        );

        for i in 0..degree_count {
            controls.push(
                ControlMeta::number(
                    format!("degree.{}", i),
                    format!("Scale degree at position {}", i),
                )
                .with_range(-127.0, 127.0)
                .with_default(self.degree(i).unwrap_or(i as i32) as f32),
            );
            controls.push(
                ControlMeta::number(
                    format!("note_weight.{}", i),
                    format!("Probability weight for degree {}", i),
                )
                .with_range(0.0, 10.0)
                .with_default(self.note_weight(i).unwrap_or(1.0)),
            );
        }

        controls
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "root_note" => Ok((self.root_note() as f32).into()),
            "degree_count" => Ok((self.degree_count() as f32).into()),
            "seed" => Ok((self.seed().unwrap_or(0) as f32).into()),
            _ => {
                if let Some(rest) = key.strip_prefix("degree.") {
                    return Ok((self
                        .degree(rest.parse::<usize>().map_err(|_| {
                            format!("Invalid degree index in control key: {}", key)
                        })?)? as f32)
                        .into());
                }
                if let Some(rest) = key.strip_prefix("note_weight.") {
                    return Ok(self
                        .note_weight(rest.parse::<usize>().map_err(|_| {
                            format!("Invalid weight index in control key: {}", key)
                        })?)?
                        .into());
                }
                Err(format!("Unknown control: {}", key))
            }
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        let value = value.as_number()?;
        match key {
            "root_note" => {
                self.set_root_note(value as u8);
                Ok(())
            }
            "degree_count" => {
                self.set_degree_count(value as usize);
                Ok(())
            }
            "seed" => {
                self.set_seed(value.max(0.0) as u64);
                Ok(())
            }
            _ => {
                if let Some(rest) = key.strip_prefix("degree.") {
                    return self.set_degree(
                        rest.parse::<usize>()
                            .map_err(|_| format!("Invalid degree index in control key: {}", key))?,
                        value as i32,
                    );
                }
                if let Some(rest) = key.strip_prefix("note_weight.") {
                    return self.set_note_weight(
                        rest.parse::<usize>()
                            .map_err(|_| format!("Invalid weight index in control key: {}", key))?,
                        value,
                    );
                }
                Err(format!("Unknown control: {}", key))
            }
        }
    }

    #[allow(private_interfaces)]
    fn set_scheduled(&self, key: &str, value: TableValue) -> Result<(), LegacyRefusal> {
        self.scheduled_write(key, value)
    }

    #[allow(private_interfaces)]
    fn get_scheduled(&self, key: &str) -> Result<f32, LegacyRefusal> {
        self.scheduled_read(key)
    }
}
