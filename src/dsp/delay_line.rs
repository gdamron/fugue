//! Pre-allocated circular delay buffer.

/// Circular delay buffer with indexed read. Pre-allocated at construction.
///
/// Supports reading at arbitrary offsets into the past and writing at the
/// current head position. All operations are O(1) with no allocation.
pub struct DelayLine {
    buffer: Vec<f32>,
    index: usize,
}

impl DelayLine {
    /// Creates a new delay line with the given length (in samples).
    pub fn new(length: usize) -> Self {
        Self {
            buffer: vec![0.0; length.max(1)],
            index: 0,
        }
    }

    /// Reads from the delay line at `offset` samples in the past.
    #[inline]
    pub fn read(&self, offset: usize) -> f32 {
        let len = self.buffer.len();
        let pos = (self.index + len - offset) % len;
        self.buffer[pos]
    }

    /// Writes a value at the current position and advances the write head.
    #[inline]
    pub fn write_and_advance(&mut self, value: f32) {
        self.buffer[self.index] = value;
        self.index += 1;
        if self.index >= self.buffer.len() {
            self.index = 0;
        }
    }
}
