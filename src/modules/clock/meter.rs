//! The clock's meter: how many beats its bars have, and which bar and beat
//! it is on.
//!
//! The clock keeps its own bar lines rather than dividing its beat count,
//! so a meter change mid-piece leaves the bars already played as they
//! were. A new meter written to a clock with one waits for the next bar
//! line; written to a clock without one, it counts bars from the latest
//! reset at once. Plain arithmetic on a few fields: nothing here allocates
//! or locks, as it runs on the audio thread.

/// A clock's bars, counted from its latest reset.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Meter {
    /// Beats in the bar playing now; 0 for no meter (then no bars).
    in_force: u32,
    /// The meter last written, which takes over at the next bar line.
    written: u32,
    /// The position the bar playing now began on.
    bar_start: f64,
    /// The bar playing now, from 1.
    bar: u32,
}

impl Meter {
    /// A meter of `beats` from beat 0 (0 for none).
    pub(super) fn new(beats: u32) -> Self {
        Self {
            in_force: beats,
            written: beats,
            bar_start: 0.0,
            bar: 1,
        }
    }

    /// Writes a meter of `beats` at `position` (negative before the clock's
    /// first sample since it was built or reset). It takes over at the next
    /// bar line, or at once when no meter is in force or the clock has not
    /// started: then bars count from beat 0 as if it had always held.
    pub(super) fn write(&mut self, beats: u32, position: f64) {
        self.advance(position);
        self.written = beats;
        if self.in_force == 0 || position < 0.0 {
            let bars = if beats == 0 {
                0.0
            } else {
                (position.max(0.0) / f64::from(beats)).floor()
            };
            self.in_force = beats;
            self.bar_start = bars * f64::from(beats);
            self.bar = (bars as u32).saturating_add(1);
        }
    }

    /// Back to bar 1 at beat 0, with the meter last written in force.
    pub(super) fn reset(&mut self) {
        *self = Self::new(self.written);
    }

    /// Moves past every bar line at or before `position`, adopting the
    /// written meter at the first.
    pub(super) fn advance(&mut self, position: f64) {
        while self.in_force > 0 {
            let line = self.bar_start + f64::from(self.in_force);
            if position < line {
                break;
            }
            self.bar_start = line;
            self.bar = self.bar.saturating_add(1);
            self.in_force = self.written;
        }
    }

    /// The `(bar, beat in the bar, meter in force)` at `position`, all 0
    /// with no meter. Call [`Self::advance`] to `position` first.
    pub(super) fn reading(&self, position: f64) -> (u32, u32, u32) {
        if self.in_force == 0 {
            return (0, 0, 0);
        }
        let into = (position.max(self.bar_start) - self.bar_start).floor();
        let beat = (into as u32).saturating_add(1).min(self.in_force);
        (self.bar, beat, self.in_force)
    }
}

#[cfg(test)]
mod tests {
    use super::Meter;

    fn at(meter: &mut Meter, position: f64) -> (u32, u32, u32) {
        meter.advance(position);
        meter.reading(position)
    }

    #[test]
    fn no_meter_counts_no_bars() {
        let mut meter = Meter::new(0);
        assert_eq!(at(&mut meter, 5.5), (0, 0, 0));
    }

    #[test]
    fn bars_count_from_one_on_their_downbeats() {
        let mut meter = Meter::new(4);
        assert_eq!(at(&mut meter, -1.0), (1, 1, 4));
        assert_eq!(at(&mut meter, 0.0), (1, 1, 4));
        assert_eq!(at(&mut meter, 3.999), (1, 4, 4));
        assert_eq!(at(&mut meter, 4.0), (2, 1, 4));
        assert_eq!(at(&mut meter, 9.5), (3, 2, 4));
    }

    #[test]
    fn a_new_meter_waits_for_the_bar_line_and_played_bars_keep_their_numbers() {
        let mut meter = Meter::new(4);
        meter.write(3, 5.5);
        assert_eq!(at(&mut meter, 7.5), (2, 4, 4));
        assert_eq!(at(&mut meter, 8.0), (3, 1, 3));
        assert_eq!(at(&mut meter, 11.0), (4, 1, 3));
    }

    #[test]
    fn a_meter_on_a_clock_without_one_counts_from_beat_zero_at_once() {
        let mut meter = Meter::new(0);
        meter.write(4, 5.5);
        assert_eq!(meter.reading(5.5), (2, 2, 4));
    }

    #[test]
    fn a_meter_of_zero_ends_the_bars_at_the_next_line() {
        let mut meter = Meter::new(4);
        meter.write(0, 2.0);
        assert_eq!(at(&mut meter, 3.5), (1, 4, 4));
        assert_eq!(at(&mut meter, 4.5), (0, 0, 0));
    }

    #[test]
    fn a_reset_adopts_the_written_meter_at_bar_one() {
        let mut meter = Meter::new(4);
        meter.write(3, 5.5);
        meter.reset();
        assert_eq!(at(&mut meter, 0.5), (1, 1, 3));
        assert_eq!(at(&mut meter, 3.5), (2, 1, 3));
    }
}
