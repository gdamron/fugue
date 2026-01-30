//! Runtime control handles for a built patch.
//!
//! Provides type-safe access to module runtime controls like tempo and melody parameters.

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

/// Collection of runtime control handles from a built patch.
///
/// Handles are stored with flat keys in the format `"module_id.handle_name"`,
/// for example `"clock.tempo"` or `"melody1.params"`.
///
/// # Example
///
/// ```rust,ignore
/// let (runtime, handles) = builder.build(patch)?;
///
/// // Get a specific handle
/// let tempo: Tempo = handles.get("clock.tempo").expect("no tempo");
///
/// // Get all handles of a type
/// let all_params: Vec<(String, MelodyParams)> = handles.all::<MelodyParams>();
///
/// // Discover available handles
/// for key in handles.keys() {
///     println!("Available: {}", key);
/// }
/// ```
pub struct PatchHandles {
    handles: HashMap<String, Arc<dyn Any + Send + Sync>>,
}

impl PatchHandles {
    /// Creates a new PatchHandles from a map of handles.
    pub(crate) fn new(handles: HashMap<String, Arc<dyn Any + Send + Sync>>) -> Self {
        Self { handles }
    }

    /// Get a handle by flat key, downcasting to the expected type.
    ///
    /// # Arguments
    ///
    /// * `key` - The flat key in format "module_id.handle_name"
    ///
    /// # Returns
    ///
    /// The handle if found and successfully downcast, None otherwise.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let tempo: Tempo = handles.get("clock.tempo").expect("no tempo");
    /// tempo.set_bpm(140.0);
    /// ```
    pub fn get<T: Clone + 'static>(&self, key: &str) -> Option<T> {
        self.handles.get(key)?.downcast_ref::<T>().cloned()
    }

    /// Get all handles of a specific type.
    ///
    /// Useful when you have multiple modules of the same type (e.g., multiple melodies).
    ///
    /// # Returns
    ///
    /// A vector of (key, handle) pairs for all handles that match the type.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let all_params: Vec<(String, MelodyParams)> = handles.all::<MelodyParams>();
    /// for (key, params) in all_params {
    ///     println!("{}: {:?}", key, params);
    /// }
    /// ```
    pub fn all<T: Clone + 'static>(&self) -> Vec<(String, T)> {
        self.handles
            .iter()
            .filter_map(|(k, v)| v.downcast_ref::<T>().map(|t| (k.clone(), t.clone())))
            .collect()
    }

    /// Get all handles matching a key prefix.
    ///
    /// Useful for getting all handles for a specific module.
    ///
    /// # Arguments
    ///
    /// * `prefix` - The prefix to match (e.g., "clock." or "melody1.")
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let clock_handles = handles.with_prefix("clock.");
    /// ```
    pub fn with_prefix(&self, prefix: &str) -> Vec<(&str, &Arc<dyn Any + Send + Sync>)> {
        self.handles
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.as_str(), v))
            .collect()
    }

    /// List all available handle keys.
    ///
    /// Useful for discovering what handles are available in a patch.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// println!("Available handles:");
    /// for key in handles.keys() {
    ///     println!("  - {}", key);
    /// }
    /// ```
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.handles.keys().map(|s| s.as_str())
    }

    /// Returns true if no handles are available.
    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }

    /// Returns the number of handles.
    pub fn len(&self) -> usize {
        self.handles.len()
    }
}
