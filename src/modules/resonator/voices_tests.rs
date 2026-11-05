use super::*;

/// How many modes and sympathetic strings a voice gets at each polyphony,
/// so the bank's cost stays about the same as voices are added.
#[test]
fn voices_share_a_fixed_budget_of_modes_and_strings() {
    let mut voices = Voices::new(48_000, 1);
    let mut modes = vec![];
    let mut strings = vec![];
    for polyphony in 1..=VOICES {
        voices.configure(0, polyphony);
        modes.push(voices.modal[0].resolution());
        strings.push(voices.sympathetic_count[0]);
    }
    assert_eq!(modes, [60, 28, 16, 12]);
    assert_eq!(strings, [8, 4, 2, 2]);
}

/// At three voices the third keeps its partials split across both outputs,
/// so the two sides stay balanced.
#[test]
fn three_voices_stay_balanced() {
    let mut voices = Voices::new(48_000, 3);
    let splits: Vec<f32> = (0..3).map(|v| voices.split[v].next_value()).collect();
    assert_eq!(splits, [1.0, 1.0, 0.0]);
}

/// A strum at a higher polyphony gives a ringing sympathetic voice fewer
/// strings; the strings it drops ring out unfed instead of vanishing, so
/// the voice's level carries straight on.
#[test]
fn dropped_sympathetic_strings_ring_out() {
    let mut voices = Voices::new(48_000, 1);
    voices.frequency[0] = 220.0 / 48_000.0;
    let patch = Patch {
        structure: 0.4,
        brightness: 0.5,
        damping: 0.9,
        position: 0.3,
    };
    let mut seed = 7u32;
    let mut level = |voices: &mut Voices, blocks: usize, feeding: bool| {
        let mut sum = 0.0f32;
        for _ in 0..blocks {
            for x in voices.excitation.iter_mut() {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *x = (seed >> 9) as f32 / 4.0e6 - 1.0;
            }
            voices.out.fill(0.0);
            voices.aux.fill(0.0);
            voices.ring_sympathetic(0, feeding, false, &patch);
            sum += voices.out.iter().map(|x| x * x).sum::<f32>();
        }
        (sum / (blocks * ENGINE_BLOCK) as f32).sqrt()
    };
    level(&mut voices, 4, true);
    level(&mut voices, 400, false);
    let before = level(&mut voices, 20, false);
    voices.configure(0, 2);
    assert_eq!(voices.sympathetic_count[0], 4);
    let after = level(&mut voices, 20, false);
    assert!(
        after >= 0.85 * before,
        "level {after} after, {before} before"
    );
}
