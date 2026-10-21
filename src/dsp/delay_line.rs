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

    /// Reads from the delay line at `offset` samples in the past, where
    /// `offset` is at most the buffer length.
    #[inline]
    pub fn read(&self, offset: usize) -> f32 {
        let len = self.buffer.len();
        debug_assert!(offset <= len, "delay offset {offset} exceeds length {len}");
        // `index + len - offset` lies in `[0, 2 * len)`, so one conditional
        // subtract wraps it without an integer divide per read.
        let pos = self.index + len - offset;
        let pos = if pos >= len { pos - len } else { pos };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_matches_modulo_wrap_for_every_offset_and_head() {
        let len = 7;
        let mut line = DelayLine::new(len);
        for step in 0..3 * len {
            for offset in 0..=len {
                let expected = line.buffer[(line.index + len - offset) % len];
                assert_eq!(line.read(offset), expected, "step {step} offset {offset}");
            }
            line.write_and_advance(step as f32);
        }
    }

    #[test]
    fn read_returns_the_sample_written_offset_steps_ago() {
        let mut line = DelayLine::new(4);
        for value in 1..=6 {
            line.write_and_advance(value as f32);
        }
        assert_eq!(line.read(1), 6.0);
        assert_eq!(line.read(2), 5.0);
        assert_eq!(line.read(4), 3.0);
    }
}
