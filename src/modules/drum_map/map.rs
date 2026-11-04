//! The pattern map: what each voice plays on a step, for a map position, a
//! density, chaos and a seed; and the Euclidean alternative.
//!
//! Every function here is pure, allocation-free and cheap (four table reads
//! and three lerps per voice and step), so the module calls them on the audio
//! thread once per step, not per sample.

use super::nodes::NODES;

/// Kick, snare and hat.
pub const VOICES: usize = 3;

/// The steps a node holds per bar: 32nds.
pub const CELLS: usize = 32;

/// Nodes per side of the map.
const SIDE: usize = 3;

/// The highest authored weight.
const TOP_WEIGHT: f32 = 9.0;

/// Hits at or above this weight (of 1) are accented: the steps a node ranks
/// near the top, the ones that hold the groove up.
pub const ACCENT_WEIGHT: f32 = 0.85;

/// The least a weighted step's weight falls to under chaos: still above 0,
/// so density 1 keeps it.
const LEAST_WEIGHT: f32 = 1.0e-3;

/// How far full chaos moves a weight, up or down.
const CHAOS_DEPTH: f32 = 0.5;

/// The weight (0 to 1) of `voice` on 32nd step `cell` at map position `x`,
/// `y` (each 0 to 1), from the four surrounding nodes.
///
/// Along each axis a node's say is its linear share doubled and capped at
/// 1; a corner's say is the product of its two, and the step takes the
/// strongest corner's weight scaled by its say. A position on a node reads
/// that node exactly; half way between nodes (along an edge or at a
/// quadrant's centre) every neighbour plays in full, so a groove's anchors
/// (a snare moving from beat 3 to beats 2 and 4) overlap through the move
/// instead of fading to half and dropping out at a moderate density.
pub fn weight(x: f32, y: f32, voice: usize, cell: usize) -> f32 {
    let (column, fx) = axis(x);
    let (row, fy) = axis(y);
    let say = |share: f32| (share * 2.0).min(1.0);
    let (left, right) = (say(1.0 - fx), say(fx));
    let (low, high) = (say(1.0 - fy), say(fy));
    let corners = [
        (row, column, left * low),
        (row, column + 1, right * low),
        (row + 1, column, left * high),
        (row + 1, column + 1, right * high),
    ];
    let mut strongest: f32 = 0.0;
    for (row, column, say) in corners {
        let authored = f32::from(NODES[row * SIDE + column][voice][cell]);
        strongest = strongest.max(authored * say);
    }
    strongest / TOP_WEIGHT
}

/// The lower node index along an axis and the fraction past it.
fn axis(value: f32) -> (usize, f32) {
    let scaled = value.clamp(0.0, 1.0) * (SIDE - 1) as f32;
    let index = (scaled as usize).min(SIDE - 2);
    (index, scaled - index as f32)
}

/// Whether `voice` plays 32nd step `cell` of bar `bar`, and if so whether
/// the hit is accented.
///
/// A step plays when its weight exceeds `1 - density`: density 0 plays
/// nothing, density 1 plays every step the position allows (weight above 0),
/// and raising the density only adds steps. `chaos` (0 to 1) moves each
/// weighted step's weight by a pseudo-random amount drawn from `seed`,
/// `bar`, `cell` and `voice`, so the same inputs always give the same
/// pattern. It varies which of the position's steps play at a given
/// density; it never adds a step the position doesn't allow nor takes one
/// away at density 1.
#[allow(clippy::too_many_arguments)]
pub fn hit(
    x: f32,
    y: f32,
    voice: usize,
    cell: usize,
    density: f32,
    chaos: f32,
    seed: u32,
    bar: u32,
) -> Option<bool> {
    let mut weight = weight(x, y, voice, cell);
    if chaos > 0.0 && weight > 0.0 {
        let noise = noise(seed, bar, cell, voice);
        weight = (weight + chaos * CHAOS_DEPTH * noise).clamp(LEAST_WEIGHT, 1.0);
    }
    (weight > 1.0 - density.clamp(0.0, 1.0)).then_some(weight >= ACCENT_WEIGHT)
}

/// A pseudo-random value in -1 to 1, a pure function of its arguments
/// (a SplitMix64 finaliser over them), so chaos needs no state.
pub fn noise(seed: u32, bar: u32, cell: usize, voice: usize) -> f32 {
    let mut z = (u64::from(seed) << 32 | u64::from(bar))
        ^ ((cell as u64) << 8 | voice as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // The top 24 bits, as a fraction of 1, then to -1..1.
    (z >> 40) as f32 / (1u64 << 23) as f32 - 1.0
}

/// Whether step `step` of a `steps`-step bar is one of `round(density ×
/// steps)` hits spread as evenly as they go (the Euclidean rhythm), with
/// step 0 always among them.
pub fn euclidean(steps: usize, density: f32, step: usize) -> bool {
    let hits = (density.clamp(0.0, 1.0) * steps as f32).round() as usize;
    hits > 0 && step * hits % steps < hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_position_on_a_node_reads_it_exactly() {
        for row in 0..SIDE {
            for column in 0..SIDE {
                let (x, y) = (column as f32 / 2.0, row as f32 / 2.0);
                for (voice, cells) in NODES[row * SIDE + column].iter().enumerate() {
                    for (cell, authored) in cells.iter().enumerate() {
                        let authored = f32::from(*authored) / TOP_WEIGHT;
                        assert_eq!(weight(x, y, voice, cell), authored);
                    }
                }
            }
        }
    }

    #[test]
    fn half_way_between_nodes_both_anchors_play_in_full() {
        // The snare on beat 3 (sparse row) and on beats 2 and 4 (standard, in
        // the straight and syncopated columns).
        for x in [0.0, 0.5] {
            for cell in [8, 16, 24] {
                assert_eq!(weight(x, 0.25, 1, cell), 1.0, "x {x} cell {cell}");
            }
        }
    }

    #[test]
    fn at_a_quadrant_centre_every_neighbour_plays_in_full() {
        for (x, y) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
            assert_eq!(weight(x, y, 0, 0), 1.0, "the downbeat kick at {x}, {y}");
            assert_eq!(hit(x, y, 0, 0, 0.5, 0.0, 0, 0), Some(true));
        }
    }

    #[test]
    fn a_step_fades_out_as_the_position_leaves_its_node() {
        // Beat 3's snare belongs to the sparse row only.
        let snare = |y| weight(0.0, y, 1, 16);
        assert_eq!(snare(0.25), 1.0);
        assert!(snare(0.375) < 1.0 && snare(0.375) > snare(0.45));
        assert_eq!(snare(0.5), 0.0);
    }

    #[test]
    fn chaos_never_adds_or_removes_a_step_at_full_density() {
        for bar in 0..32 {
            for cell in 0..CELLS {
                for voice in 0..VOICES {
                    let allowed = weight(0.7, 0.2, voice, cell) > 0.0;
                    let hit = hit(0.7, 0.2, voice, cell, 1.0, 1.0, 9, bar);
                    assert_eq!(hit.is_some(), allowed);
                }
            }
        }
    }

    #[test]
    fn every_node_puts_a_kick_on_the_downbeat_at_the_lowest_density() {
        for node in NODES {
            assert_eq!(node[0][0], 9);
        }
    }

    #[test]
    fn density_zero_is_silent_and_one_plays_every_weighted_step() {
        for cell in 0..CELLS {
            for voice in 0..VOICES {
                assert_eq!(hit(0.3, 0.8, voice, cell, 0.0, 1.0, 7, 2), None);
                let allowed = weight(0.3, 0.8, voice, cell) > 0.0;
                let full = hit(0.3, 0.8, voice, cell, 1.0, 0.0, 0, 0);
                assert_eq!(full.is_some(), allowed, "voice {voice} cell {cell}");
            }
        }
    }

    #[test]
    fn noise_spans_minus_one_to_one_and_repeats() {
        let values: Vec<f32> = (0..1000).map(|bar| noise(3, bar, 5, 1)).collect();
        assert!(values.iter().all(|v| (-1.0..1.0).contains(v)));
        assert!(values.iter().any(|v| *v < -0.9) && values.iter().any(|v| *v > 0.9));
        assert_eq!(noise(3, 17, 5, 1), noise(3, 17, 5, 1));
        assert_ne!(noise(3, 17, 5, 1), noise(4, 17, 5, 1));
    }

    #[test]
    fn euclidean_spreads_its_hits_from_the_downbeat() {
        let bar =
            |density| -> Vec<usize> { (0..16).filter(|&s| euclidean(16, density, s)).collect() };
        assert_eq!(bar(0.0), Vec::<usize>::new());
        assert_eq!(bar(0.25), vec![0, 4, 8, 12]);
        assert_eq!(bar(0.1875), vec![0, 6, 11]);
        assert_eq!(bar(1.0).len(), 16);
    }
}
