use std::sync::{Arc, Mutex};

/// A thread-safe container for user-controlled parameters.
///
/// Unlike [`Audio`](super::Audio), this is not an audio-rate signal.
/// It represents user interaction with the system such as knob positions,
/// button states, or MIDI CC values.
///
/// `Control` can be safely read and written from both the audio thread
/// and UI thread simultaneously.
#[derive(Clone)]
pub struct Control<T> {
    value: Arc<Mutex<T>>,
}

impl<T> Control<T> {
    /// Creates a new control with the given initial value.
    pub fn new(value: T) -> Self {
        Self {
            value: Arc::new(Mutex::new(value)),
        }
    }

    /// Returns a copy of the current value.
    pub fn get(&self) -> T
    where
        T: Copy,
    {
        *self.value.lock().unwrap()
    }

    /// Sets the control to a new value.
    pub fn set(&self, new_value: T) {
        *self.value.lock().unwrap() = new_value;
    }

    /// Reads the value by applying a function to it.
    ///
    /// Useful when the value type doesn't implement `Copy`.
    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        f(&*self.value.lock().unwrap())
    }

    /// Modifies the value in place using a closure.
    pub fn modify<F>(&self, f: F)
    where
        F: FnOnce(&mut T),
    {
        f(&mut *self.value.lock().unwrap())
    }

    /// Returns a clone of the underlying `Arc<Mutex<T>>` for sharing.
    pub fn inner(&self) -> Arc<Mutex<T>> {
        Arc::clone(&self.value)
    }
}
