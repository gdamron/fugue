use std::sync::{Arc, Mutex};

/// Control - human input and parameter changes
/// NOT an audio-rate signal - represents user interaction with the system.
///
/// Examples:
/// - Knob positions
/// - Button states
/// - Switch positions
/// - Key presses
/// - Parameter automation values
/// - MIDI CC values
///
/// These are thread-safe and can be read/written from the audio thread
/// and UI thread simultaneously.
#[derive(Clone)]
pub struct Control<T> {
    value: Arc<Mutex<T>>,
}

impl<T> Control<T> {
    pub fn new(value: T) -> Self {
        Self {
            value: Arc::new(Mutex::new(value)),
        }
    }

    pub fn get(&self) -> T
    where
        T: Copy,
    {
        *self.value.lock().unwrap()
    }

    pub fn set(&self, new_value: T) {
        *self.value.lock().unwrap() = new_value;
    }

    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        f(&*self.value.lock().unwrap())
    }

    pub fn modify<F>(&self, f: F)
    where
        F: FnOnce(&mut T),
    {
        f(&mut *self.value.lock().unwrap())
    }

    pub fn inner(&self) -> Arc<Mutex<T>> {
        Arc::clone(&self.value)
    }
}
