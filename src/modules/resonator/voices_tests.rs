use super::*;

/// How many modes a voice gets at each polyphony, so the bank's cost stays
/// about the same as voices are added.
#[test]
fn voices_share_a_fixed_budget_of_modes_and_strings() {
    let mut voices = Voices::new(48_000, 1);
    let mut modes = vec![];
    for polyphony in 1..=VOICES {
        voices.configure(0, polyphony);
        modes.push(voices.modal[0].resolution());
    }
    assert_eq!(modes, [60, 28, 16, 12]);
}

/// At three voices the third keeps its partials split across both outputs,
/// so the two sides stay balanced.
#[test]
fn three_voices_stay_balanced() {
    let mut voices = Voices::new(48_000, 3);
    let splits: Vec<f32> = (0..3).map(|v| voices.split[v].next_value()).collect();
    assert_eq!(splits, [1.0, 1.0, 0.0]);
}
