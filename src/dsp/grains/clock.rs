//! Rising edges and clock divisions, read from gate inputs a frame at a
//! time.

/// A rising edge: a frame above 0.5 after one at or below it.
#[derive(Default)]
pub struct RisingEdge {
    high: bool,
}

impl RisingEdge {
    #[inline]
    pub fn rose(&mut self, gate: f32) -> bool {
        let high = gate > 0.5;
        let rose = high && !self.high;
        self.high = high;
        rose
    }
}

/// Divides each clock pulse into evenly spaced onsets: one on the edge,
/// and the rest spaced by the last pulse's length (so the first pulse,
/// with nothing measured yet, gets its edge alone).
#[derive(Default)]
pub struct ClockDivider {
    edge: RisingEdge,
    /// Frames since the last edge, if there has been one.
    since: Option<u32>,
    /// The last pulse's length in frames, once two edges have passed.
    period: Option<u32>,
    /// Onsets this pulse is divided into, and how many have fallen.
    divisions: u32,
    fallen: u32,
}

impl ClockDivider {
    /// Whether an onset falls on this frame of `gate`, dividing pulses into
    /// `divisions`. The division count is taken at each edge.
    #[inline]
    pub fn tick(&mut self, gate: f32, divisions: u32) -> bool {
        if self.edge.rose(gate) {
            if let Some(since) = self.since {
                self.period = Some(since.saturating_add(1));
            }
            self.since = Some(0);
            self.divisions = divisions.max(1);
            self.fallen = 1;
            return true;
        }
        let Some(since) = self.since.as_mut() else {
            return false;
        };
        *since = since.saturating_add(1);
        let Some(period) = self.period else {
            return false;
        };
        if self.fallen >= self.divisions {
            return false;
        }
        let due = (u64::from(self.fallen) * u64::from(period) * 2 + u64::from(self.divisions))
            / (2 * u64::from(self.divisions));
        if u64::from(*since) >= due {
            self.fallen += 1;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn onsets(period: usize, pulses: usize, divisions: u32) -> Vec<usize> {
        let mut divider = ClockDivider::default();
        (0..period * pulses)
            .filter(|n| {
                let gate = if n % period < period / 2 { 1.0 } else { 0.0 };
                divider.tick(gate, divisions)
            })
            .collect()
    }

    #[test]
    fn the_first_pulse_has_its_edge_alone_then_each_divides_evenly() {
        let got = onsets(1200, 3, 4);
        assert_eq!(got, [0, 1200, 1500, 1800, 2100, 2400, 2700, 3000, 3300]);
    }

    #[test]
    fn a_division_that_does_not_fit_rounds_to_the_nearest_frame() {
        let got = onsets(1000, 2, 3);
        assert_eq!(got, [0, 1000, 1333, 1667]);
    }
}
