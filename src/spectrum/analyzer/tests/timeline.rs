//! An exact record of what reached a tap, for checking an analyser's output
//! frame by frame rather than within tolerances.

use crate::rpc::SpectrogramTile;
use crate::spectrum::SpectrumTap;

/// Feeds audio to a tap while recording, from the tap's own counts, which
/// stretches of the stream arrived and which were lost.
///
/// Within one block the tap accepts a prefix and drops the rest, so each
/// block's counts place its loss exactly.
pub(super) struct Timeline {
    tap: SpectrumTap,
    /// Where the next sample sits in the stream, lost audio included.
    position: u64,
    /// Stretches of arrived audio as half-open `[start, end)` ranges.
    arrived: Vec<(u64, u64)>,
}

impl Timeline {
    /// Starts recording at the current stream's position zero.
    pub(super) fn new(tap: &SpectrumTap) -> Self {
        Self {
            tap: tap.clone(),
            position: 0,
            arrived: Vec::new(),
        }
    }

    /// Hands `samples` of silence to the tap and records what became of them.
    pub(super) fn feed(&mut self, samples: usize) {
        let accepted_before = self.tap.accepted_total();
        let dropped_before = self.tap.dropped_total();
        let block = vec![0.0; samples];
        self.tap.observe_block(&block, &block, block.len());
        let accepted = self.tap.accepted_total() - accepted_before;
        let dropped = self.tap.dropped_total() - dropped_before;
        assert_eq!(
            accepted + dropped,
            samples as u64,
            "every sample is accounted for"
        );

        if accepted > 0 {
            let end = self.position + accepted;
            match self.arrived.last_mut() {
                Some(last) if last.1 == self.position => last.1 = end,
                _ => self.arrived.push((self.position, end)),
            }
        }
        self.position += accepted + dropped;
    }

    /// The frames whose whole window arrived without a loss inside it: the
    /// frames an analyser should produce once it has read everything.
    pub(super) fn expected_frames(&self, fft_size: u64, hop: u64) -> Vec<u64> {
        let mut frames = Vec::new();
        for &(start, end) in &self.arrived {
            let mut frame = start.div_ceil(hop);
            while frame * hop + fft_size <= end {
                frames.push(frame);
                frame += 1;
            }
        }
        frames
    }

    /// Total samples lost so far.
    pub(super) fn lost(&self) -> u64 {
        self.position - self.arrived.iter().map(|(s, e)| e - s).sum::<u64>()
    }
}

/// Every frame index the tiles carry, in order.
pub(super) fn frames_in(tiles: &[SpectrogramTile]) -> Vec<u64> {
    tiles
        .iter()
        .flat_map(|t| t.start_frame..t.start_frame + t.frame_count as u64)
        .collect()
}
