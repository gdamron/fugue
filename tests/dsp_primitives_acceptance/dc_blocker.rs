use crate::gain_db;
use fugue::dsp::{DcBlocker, DC_BLOCKER_CUTOFF_HZ};

#[test]
fn removes_half_scale_offset_within_100_ms() {
    for rate in [44_100u32, 48_000, 96_000] {
        let mut dc = DcBlocker::new(rate);
        for n in 0..rate as usize {
            let y = dc.process(0.5);
            assert!(
                n < (rate / 10) as usize || y.abs() < 1e-3,
                "{rate} Hz: {y} at {n}"
            );
        }
    }
}

#[test]
fn minus_3_db_point_is_between_5_and_20_hz() {
    assert!((5.0..=20.0).contains(&DC_BLOCKER_CUTOFF_HZ));
    for rate in [44_100u32, 48_000] {
        let gain = |f| {
            let mut dc = DcBlocker::new(rate);
            gain_db(rate, f, move |x| dc.process(x))
        };
        // 0.05 dB of slack at the 5 and 20 Hz boundaries.
        assert!(gain(5.0) <= -3.0 + 0.05, "{rate}: 5 Hz {} dB", gain(5.0));
        assert!(gain(20.0) >= -3.0 - 0.05, "{rate}: 20 Hz {} dB", gain(20.0));
        // Assumption: audio passes (within 0.1 dB at 1 kHz).
        assert!(gain(1000.0) > -0.1, "{rate}: 1 kHz {} dB", gain(1000.0));
    }
}
