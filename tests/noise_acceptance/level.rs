//! The `level` control.

use super::rig::*;

/// Criterion: level scales output between silence and full. Half level
/// is audibly quieter than full and louder than silence, on both outputs, and
/// restoring level 1 restores the full statistics (white RMS 0.577).
/// Assumption: the taper is not specified, so only monotonic ordering is
/// checked, not that 0.5 gives half the amplitude.
#[test]
fn intermediate_level_scales_between_silence_and_full() {
    let mut n = seeded(7);
    n.run(seconds(0.1));
    let (full_w, full_p) = n.render(seconds(2.0));
    n.set_level(0.5);
    n.run(seconds(0.1));
    let (half_w, half_p) = n.render(seconds(2.0));
    assert!(
        rms(&half_w) > 0.01 && rms(&half_w) < 0.95 * rms(&full_w),
        "white {}",
        rms(&half_w)
    );
    assert!(
        rms(&half_p) > 0.0 && rms(&half_p) < 0.95 * rms(&full_p),
        "pink {}",
        rms(&half_p)
    );
    n.set_level(1.0);
    n.run(seconds(0.1));
    let (back_w, _) = n.render(seconds(2.0));
    assert!(
        (rms(&back_w) - 0.577).abs() <= 0.02,
        "restored rms {}",
        rms(&back_w)
    );
}
