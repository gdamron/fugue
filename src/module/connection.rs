/// Connection - represents a signal path between modules
/// This is like patching cables in Eurorack
pub struct Connection<T> {
    signal: T,
}

impl<T: Clone> Connection<T> {
    pub fn new(signal: T) -> Self {
        Self { signal }
    }

    pub fn read(&self) -> T {
        self.signal.clone()
    }

    pub fn write(&mut self, signal: T) {
        self.signal = signal;
    }
}
