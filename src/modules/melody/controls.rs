//! Thread-safe controls for the MelodyGenerator module.

use std::sync::{Arc, Mutex};

use crate::{ControlMeta, ControlSurface, ControlValue};

/// Thread-safe controls for the MelodyGenerator module.
///
/// All fields are wrapped in `Arc<Mutex<_>>` for real-time adjustment
/// from any thread while audio is playing.
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
    /// Scale degrees that can be selected for notes.
    pub(crate) allowed_degrees: Arc<Mutex<Vec<usize>>>,
    /// Probability weights for each allowed degree.
    pub(crate) note_weights: Arc<Mutex<Vec<f32>>>,
}

impl MelodyControls {
    /// Creates new melody controls with the given allowed scale degrees.
    ///
    /// All degrees start with equal probability weight.
    /// Oscillator type defaults to sine.
    pub fn new(allowed_degrees: Vec<usize>) -> Self {
        let weights = vec![1.0; allowed_degrees.len()];
        Self {
            allowed_degrees: Arc::new(Mutex::new(allowed_degrees)),
            note_weights: Arc::new(Mutex::new(weights)),
        }
    }

    /// Gets the allowed scale degrees.
    pub fn allowed_degrees(&self) -> Vec<usize> {
        self.allowed_degrees.lock().unwrap().clone()
    }

    /// Sets which scale degrees can be used for note selection.
    ///
    /// Also resizes the weights vector to match.
    pub fn set_allowed_degrees(&self, degrees: Vec<usize>) {
        let mut allowed = self.allowed_degrees.lock().unwrap();
        *allowed = degrees.clone();

        let mut weights = self.note_weights.lock().unwrap();
        weights.resize(degrees.len(), 1.0);
    }

    /// Gets the note weights.
    pub fn note_weights(&self) -> Vec<f32> {
        self.note_weights.lock().unwrap().clone()
    }

    /// Sets the probability weights for note selection.
    ///
    /// Higher weights make that degree more likely to be chosen.
    pub fn set_note_weights(&self, weights: Vec<f32>) {
        *self.note_weights.lock().unwrap() = weights;
    }

    /// Gets the number of allowed degrees.
    pub fn degree_count(&self) -> usize {
        self.allowed_degrees.lock().unwrap().len()
    }

    /// Sets the number of allowed degrees.
    ///
    /// If growing, new entries use sequential degree indices and weight 1.0.
    /// If shrinking, truncates both allowed_degrees and note_weights.
    pub fn set_degree_count(&self, count: usize) {
        let count = count.clamp(1, 128);
        let mut degrees = self.allowed_degrees.lock().unwrap();
        let mut weights = self.note_weights.lock().unwrap();

        let old_len = degrees.len();
        if count > old_len {
            // Append sequential degrees starting after the last existing degree
            let next_degree = degrees.last().map(|&d| d + 1).unwrap_or(0);
            for i in 0..(count - old_len) {
                degrees.push(next_degree + i);
            }
            weights.resize(count, 1.0);
        } else {
            degrees.truncate(count);
            weights.truncate(count);
        }
    }

    /// Gets the scale degree at position `index`.
    pub fn degree(&self, index: usize) -> Result<usize, String> {
        let degrees = self.allowed_degrees.lock().unwrap();
        degrees.get(index).copied().ok_or_else(|| {
            format!(
                "Degree index {} out of range (count: {})",
                index,
                degrees.len()
            )
        })
    }

    /// Sets the scale degree at position `index`.
    pub fn set_degree(&self, index: usize, value: usize) -> Result<(), String> {
        let mut degrees = self.allowed_degrees.lock().unwrap();
        if index >= degrees.len() {
            return Err(format!(
                "Degree index {} out of range (count: {})",
                index,
                degrees.len()
            ));
        }
        degrees[index] = value.min(127);
        Ok(())
    }

    /// Gets the note weight at position `index`.
    pub fn note_weight(&self, index: usize) -> Result<f32, String> {
        let weights = self.note_weights.lock().unwrap();
        weights.get(index).copied().ok_or_else(|| {
            format!(
                "Weight index {} out of range (count: {})",
                index,
                weights.len()
            )
        })
    }

    /// Sets the note weight at position `index`.
    pub fn set_note_weight(&self, index: usize, value: f32) -> Result<(), String> {
        let mut weights = self.note_weights.lock().unwrap();
        if index >= weights.len() {
            return Err(format!(
                "Weight index {} out of range (count: {})",
                index,
                weights.len()
            ));
        }
        weights[index] = value.clamp(0.0, 10.0);
        Ok(())
    }
}

// Provide a type alias for backward compatibility
/// Deprecated: Use [`MelodyControls`] instead.
#[deprecated(since = "0.2.0", note = "Renamed to MelodyControls for consistency")]
pub type MelodyParams = MelodyControls;

impl ControlSurface for MelodyControls {
    fn controls(&self) -> Vec<ControlMeta> {
        let degree_count = self.degree_count();
        let mut controls = Vec::with_capacity(2 + degree_count * 2);
        controls.push(
            ControlMeta::number("degree_count", "Number of active scale degrees")
                .with_range(1.0, 128.0)
                .with_default(degree_count as f32),
        );

        for i in 0..degree_count {
            controls.push(
                ControlMeta::number(
                    format!("degree.{}", i),
                    format!("Scale degree at position {}", i),
                )
                .with_range(0.0, 127.0)
                .with_default(self.degree(i).unwrap_or(i) as f32),
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
            "degree_count" => Ok((self.degree_count() as f32).into()),
            _ => {
                if let Some(rest) = key.strip_prefix("degree.") {
                    return Ok((self.degree(rest.parse::<usize>().map_err(|_| {
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
            "degree_count" => {
                self.set_degree_count(value as usize);
                Ok(())
            }
            _ => {
                if let Some(rest) = key.strip_prefix("degree.") {
                    return self.set_degree(
                        rest.parse::<usize>()
                            .map_err(|_| format!("Invalid degree index in control key: {}", key))?,
                        value as usize,
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
