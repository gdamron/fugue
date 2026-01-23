/// A buffer holding a signal value between modules.
///
/// Represents a virtual patch cable connecting module outputs to inputs.
pub struct Connection<T> {
    signal: T,
}

impl<T: Clone> Connection<T> {
    /// Creates a new connection with an initial signal value.
    pub fn new(signal: T) -> Self {
        Self { signal }
    }

    /// Reads the current signal value from this connection.
    pub fn read(&self) -> T {
        self.signal.clone()
    }

    /// Writes a new signal value to this connection.
    pub fn write(&mut self, signal: T) {
        self.signal = signal;
    }
}
