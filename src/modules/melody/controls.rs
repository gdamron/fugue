//! Thread-safe controls for the MelodyGenerator module.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use crate::{ControlMeta, ControlSurface, ControlValue};

/// Maximum number of scale degrees a melody can choose between.
///
/// Bounds the audio thread's pre-allocated copy of the degree table.
pub(crate) const MAX_DEGREES: usize = 128;

/// Scale degrees and their selection weights, edited together under one lock.
///
/// What the melody plays (`degrees`, `weights`) is computed from four
/// sources: the base `scale` and `scale_weights`, the active `count`, and the
/// values written to single positions. So the same sources always give the
/// same table, whatever order they were set in, and a document that records
/// them rebuilds exactly what was playing.
///
/// - Shrinking the count hides the positions past it; growing it again shows
///   them as they were, written values included.
/// - Growing past the scale repeats it: position `i` plays `scale[i % n]`
///   with weight `scale_weights[i % n]`, unless a weight is given for `i`.
///   An empty scale grows with degree 0.
///
/// Every buffer is allocated at construction with room for [`MAX_DEGREES`]
/// positions, so a count or single-position write never allocates: a
/// `control_scheduler` ramp may make one per audio sample.
pub(crate) struct DegreeTable {
    /// Scale degrees (semitone offsets) that can be selected for notes, one
    /// per active position. Negative values go below the root note.
    pub(crate) degrees: Vec<i32>,
    /// Probability weight for each active position.
    pub(crate) weights: Vec<f32>,
    /// The base scale, as configured or set whole.
    scale: Vec<i32>,
    /// The base weights, as configured or set whole; may be shorter or longer
    /// than `scale`.
    scale_weights: Vec<f32>,
    /// Number of active positions.
    count: usize,
    /// Degrees written to single positions, kept while hidden.
    written_degrees: Box<[Option<i32>; MAX_DEGREES]>,
    /// Weights written to single positions, kept while hidden.
    written_weights: Box<[Option<f32>; MAX_DEGREES]>,
}

impl DegreeTable {
    fn new(mut scale: Vec<i32>) -> Self {
        scale.truncate(MAX_DEGREES);
        let mut table = Self {
            degrees: Vec::with_capacity(MAX_DEGREES),
            weights: Vec::with_capacity(MAX_DEGREES),
            count: scale.len(),
            scale,
            scale_weights: Vec::new(),
            written_degrees: Box::new([None; MAX_DEGREES]),
            written_weights: Box::new([None; MAX_DEGREES]),
        };
        table.recompute();
        table
    }

    /// The degree position `i` plays.
    fn degree_at(&self, i: usize) -> i32 {
        let n = self.scale.len();
        self.written_degrees[i].unwrap_or(if n == 0 { 0 } else { self.scale[i % n] })
    }

    /// The weight position `i` takes.
    fn weight_at(&self, i: usize) -> f32 {
        let n = self.scale.len();
        self.written_weights[i]
            .or_else(|| self.scale_weights.get(i).copied())
            .or_else(|| {
                (n > 0)
                    .then(|| self.scale_weights.get(i % n).copied())
                    .flatten()
            })
            .unwrap_or(1.0)
    }

    /// Recomputes the active degrees and weights from the sources, within
    /// the buffers' capacity.
    fn recompute(&mut self) {
        self.degrees.clear();
        self.weights.clear();
        for i in 0..self.count {
            let (degree, weight) = (self.degree_at(i), self.weight_at(i));
            self.degrees.push(degree);
            self.weights.push(weight);
        }
    }

    fn out_of_range(&self, what: &str, index: usize) -> String {
        format!("{what} index {index} out of range (count: {})", self.count)
    }
}

/// Thread-safe controls for the MelodyGenerator module.
///
/// Scalar fields are atomics. The degree table (degrees + weights) lives
/// behind a `Mutex` gated by an atomic version counter: the audio thread keeps
/// its own pre-allocated copy and only `try_lock`s the table when the version
/// has changed, so control edits never block the audio callback.
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
    pub(crate) table: Arc<Mutex<DegreeTable>>,
    /// Bumped (while holding `table`) after every table edit; the audio
    /// thread re-copies the table when it observes a change.
    pub(crate) table_version: Arc<AtomicU64>,
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
            table: Arc::new(Mutex::new(DegreeTable::new(allowed_degrees))),
            table_version: Arc::new(AtomicU64::new(0)),
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
        self.table_version.load(Ordering::Acquire)
    }

    /// Applies `edit` to the degree table and publishes the change to the
    /// audio thread. The version is bumped while the lock is still held, so
    /// a reader that observes the new version always copies the edited table.
    fn edit_table<R>(&self, edit: impl FnOnce(&mut DegreeTable) -> R) -> R {
        let mut table = self.table.lock().unwrap();
        let result = edit(&mut table);
        self.table_version.fetch_add(1, Ordering::Release);
        result
    }

    /// Gets the active scale degrees.
    pub fn allowed_degrees(&self) -> Vec<i32> {
        self.table.lock().unwrap().degrees.clone()
    }

    /// Sets the base scale: which degrees can be used for note selection.
    ///
    /// The count becomes its length, and degrees written to single positions
    /// are forgotten; weights are kept. At most [`MAX_DEGREES`] degrees are
    /// kept.
    pub fn set_allowed_degrees(&self, mut degrees: Vec<i32>) {
        degrees.truncate(MAX_DEGREES);
        self.edit_table(|table| {
            table.count = degrees.len();
            table.scale = degrees;
            table.written_degrees.fill(None);
            table.recompute();
        });
    }

    /// Gets the active note weights.
    pub fn note_weights(&self) -> Vec<f32> {
        self.table.lock().unwrap().weights.clone()
    }

    /// Sets the base probability weights for note selection, forgetting
    /// weights written to single positions.
    ///
    /// Higher weights make that degree more likely to be chosen. A position
    /// without a weight of its own takes the weight of the scale position it
    /// repeats, or 1.0.
    pub fn set_note_weights(&self, weights: Vec<f32>) {
        self.edit_table(|table| {
            table.scale_weights = weights;
            table.written_weights.fill(None);
            table.recompute();
        });
    }

    /// Gets the number of active degrees.
    pub fn degree_count(&self) -> usize {
        self.table.lock().unwrap().degrees.len()
    }

    /// Sets the number of active degrees (1 to [`MAX_DEGREES`]).
    ///
    /// Shrinking hides the positions past the count; growing shows them again
    /// as they were. Positions past the base scale repeat it from the start.
    pub fn set_degree_count(&self, count: usize) {
        let count = count.clamp(1, MAX_DEGREES);
        // A ramp writes the count every sample; only a change is an edit.
        let mut table = self.table.lock().unwrap();
        if table.count == count {
            return;
        }
        table.count = count;
        table.recompute();
        self.table_version.fetch_add(1, Ordering::Release);
    }

    /// Gets the scale degree at active position `index`.
    pub fn degree(&self, index: usize) -> Result<i32, String> {
        let table = self.table.lock().unwrap();
        table
            .degrees
            .get(index)
            .copied()
            .ok_or_else(|| table.out_of_range("Degree", index))
    }

    /// Sets the scale degree at active position `index` (clamped to ±127).
    pub fn set_degree(&self, index: usize, value: i32) -> Result<(), String> {
        self.edit_table(|table| {
            if index >= table.degrees.len() {
                return Err(table.out_of_range("Degree", index));
            }
            let value = value.clamp(-127, 127);
            table.written_degrees[index] = Some(value);
            table.degrees[index] = value;
            Ok(())
        })
    }

    /// Gets the note weight at active position `index`.
    pub fn note_weight(&self, index: usize) -> Result<f32, String> {
        let table = self.table.lock().unwrap();
        table
            .weights
            .get(index)
            .copied()
            .ok_or_else(|| table.out_of_range("Weight", index))
    }

    /// Sets the note weight at active position `index` (clamped to 0-10).
    pub fn set_note_weight(&self, index: usize, value: f32) -> Result<(), String> {
        self.edit_table(|table| {
            if index >= table.weights.len() {
                return Err(table.out_of_range("Weight", index));
            }
            let value = value.clamp(0.0, 10.0);
            table.written_weights[index] = Some(value);
            table.weights[index] = value;
            Ok(())
        })
    }

    /// Restores a degree and a weight written to single positions, as a
    /// document records them, whether or not the position is active now: a
    /// hidden one shows again when the count grows. For building only.
    pub(crate) fn restore_written(
        &self,
        degrees: impl IntoIterator<Item = (usize, i32)>,
        weights: impl IntoIterator<Item = (usize, f32)>,
    ) {
        self.edit_table(|table| {
            for (index, value) in degrees {
                if index < MAX_DEGREES {
                    table.written_degrees[index] = Some(value.clamp(-127, 127));
                }
            }
            for (index, value) in weights {
                if index < MAX_DEGREES {
                    table.written_weights[index] = Some(value.clamp(0.0, 10.0));
                }
            }
            table.recompute();
        });
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
}
