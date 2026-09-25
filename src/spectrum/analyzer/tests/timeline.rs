//! An exact account of which frames an analyser should have produced, for
//! checking its output frame by frame rather than within tolerances.
//!
//! The tap's own tests prove that every loss a reader reports is exact. Given
//! those losses, this works out which frames had their whole window arrive,
//! so the analyser's tests only have to prove it placed frames around them.

use super::super::SpectrumAnalyzer;
use crate::rpc::SpectrogramTile;

/// Every frame whose window lies wholly within audio the analyser has taken
/// in, between the losses it met: what it should have produced by now.
pub(super) fn expected_frames(analyzer: &SpectrumAnalyzer) -> Vec<u64> {
    let size = analyzer.config.fft_size as u64;
    let hop = analyzer.config.hop_size as u64;
    let consumed = analyzer.reader.position() - analyzer.staged.len() as u64;

    let mut frames = Vec::new();
    let mut arrived_from = 0u64;
    let ends = analyzer
        .losses
        .iter()
        .copied()
        .chain(std::iter::once((consumed, 0)));
    for (lost_at, lost) in ends {
        let mut frame = arrived_from.div_ceil(hop);
        while frame * hop + size <= lost_at {
            frames.push(frame);
            frame += 1;
        }
        arrived_from = lost_at + lost;
    }
    frames
}

/// Every frame index the tiles carry, in order.
pub(super) fn frames_in(tiles: &[SpectrogramTile]) -> Vec<u64> {
    tiles
        .iter()
        .flat_map(|t| t.start_frame..t.start_frame + t.frame_count as u64)
        .collect()
}
