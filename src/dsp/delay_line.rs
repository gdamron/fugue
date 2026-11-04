//! Pre-allocated circular delay buffer.

/// Circular delay buffer with indexed read. Pre-allocated at construction.
///
/// Supports reading at whole or fractional offsets into the past and
/// writing at the current head position. All operations are O(1) with no
/// allocation.
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
        // With `offset` clamped to `len`, `index + len - offset` lies in
        // `[0, 2 * len)`, so one conditional subtract wraps it without an
        // integer divide per read.
        let pos = self.index + len - offset.min(len);
        let pos = if pos >= len { pos - len } else { pos };
        self.buffer[pos]
    }

    /// The number of samples the line holds: the longest offset it reads.
    #[inline]
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Whether the line holds no samples. Never true: a line holds at least
    /// one.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Reads between samples: `offset` samples in the past, where `offset`
    /// may fall between two whole samples. A whole offset reads that sample
    /// exactly, bit for bit, as [`DelayLine::read`] does. Between samples it
    /// interpolates with a four-point cubic (Catmull-Rom) through the
    /// samples either side, so a read head that moves smoothly (chorus,
    /// tape wow) neither steps nor dulls the highs the way a straight line
    /// between two samples does. Offsets below 2 interpolate linearly, as
    /// the newer neighbour a cubic needs has not been written yet.
    ///
    /// `offset` is clamped to `[1, len - 2]`, the span every neighbour
    /// read stays inside.
    #[inline]
    pub fn read_fractional(&self, offset: f32) -> f32 {
        let longest = self.buffer.len().saturating_sub(2).max(1) as f32;
        let offset = if offset.is_finite() {
            offset.clamp(1.0, longest)
        } else {
            1.0
        };
        let whole = offset.floor();
        let t = offset - whole;
        let i = whole as usize;
        if t == 0.0 {
            return self.read(i);
        }
        // `i + 1` is older than `i`: the read moves back in time as `t`
        // grows.
        let (near, far) = (self.read(i), self.read(i + 1));
        if i < 2 {
            return near + t * (far - near);
        }
        let newer = self.read(i - 1);
        let older = self.read(i + 2);
        let c1 = 0.5 * (far - newer);
        let c2 = newer - 2.5 * near + 2.0 * far - 0.5 * older;
        let c3 = 0.5 * (older - newer) + 1.5 * (near - far);
        ((c3 * t + c2) * t + c1) * t + near
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

    fn line_of(values: &[f32], len: usize) -> DelayLine {
        let mut line = DelayLine::new(len);
        for &value in values {
            line.write_and_advance(value);
        }
        line
    }

    #[test]
    fn a_whole_fractional_offset_reads_that_sample_bit_for_bit() {
        let values: Vec<f32> = (0..32).map(|n| (n as f32 * 0.37).sin()).collect();
        let line = line_of(&values, 32);
        for offset in 1..=30 {
            assert_eq!(
                line.read_fractional(offset as f32).to_bits(),
                line.read(offset).to_bits(),
                "offset {offset}"
            );
        }
    }

    #[test]
    fn between_samples_a_ramp_reads_on_the_line_through_them() {
        // A cubic through four points of a straight line is that line, so
        // a ramp shows where the read lands.
        let values: Vec<f32> = (0..16).map(|n| n as f32).collect();
        let line = line_of(&values, 16);
        // The newest sample (15.0) is at offset 1; offset 3.25 is a quarter
        // of the way from offset 3 (13.0) to offset 4 (12.0).
        assert!((line.read_fractional(3.25) - 12.75).abs() < 1e-5);
        assert!((line.read_fractional(1.5) - 14.5).abs() < 1e-5);
        assert!((line.read_fractional(7.9) - 8.1).abs() < 1e-4);
    }

    #[test]
    fn a_fractional_read_is_continuous_as_the_offset_sweeps() {
        let values: Vec<f32> = (0..256).map(|n| (n as f32 * 0.21).sin()).collect();
        let line = line_of(&values, 256);
        let mut last = line.read_fractional(1.0);
        let mut offset = 1.0f32;
        while offset < 200.0 {
            offset += 0.01;
            let now = line.read_fractional(offset);
            // The signal moves at most 0.21 per sample, so 0.01 of a
            // sample moves it at most about 0.0021 (cubic overshoot aside).
            assert!(
                (now - last).abs() < 0.004,
                "offset {offset}: {last} -> {now}"
            );
            last = now;
        }
    }

    #[test]
    fn a_fractional_offset_outside_the_line_is_clamped_into_it() {
        let values: Vec<f32> = (0..8).map(|n| n as f32).collect();
        let line = line_of(&values, 8);
        assert_eq!(line.read_fractional(0.0), line.read(1));
        assert_eq!(line.read_fractional(f32::NAN), line.read(1));
        assert_eq!(line.read_fractional(100.0), line.read(6));
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
