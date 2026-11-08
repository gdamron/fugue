//! Criterion: the peak meter still reports the pre-clamp peak, as it does
//! live. The offline engine has no meter, so this reads the live master
//! meter, headless through a capturing backend.

use super::support::*;

#[test]
fn the_master_meter_reports_the_pre_clamp_peak() {
    // +12 dB on a full-scale sine peaks near 3.98 (10^(12/20)).
    let live = Live::start(&tone("sine", 440.0, 12.0));
    let graph_output = live.capture(9_600);
    let levels = live.running.master_meter();
    live.running.stop();
    let peak = graph_output.iter().fold(0.0f32, |a, x| a.max(x.abs()));
    assert!(peak > 3.5 && peak < 4.2, "graph peak {peak}");
    for (side, reported) in [("left", levels.left_peak), ("right", levels.right_peak)] {
        assert!(
            reported > 3.5 && reported < 4.2,
            "{side} peak is the pre-clamp {reported}, not 1.0"
        );
        assert!(
            (reported - peak).abs() < 1e-3,
            "{side}: {reported} vs {peak}"
        );
    }
}

#[test]
fn an_offline_render_beforehand_does_not_change_the_live_meter() {
    // Rendering the same invention offline in this process leaves
    // the live meter's reading as it was.
    let json = tone("sine", 440.0, 12.0);
    let live = Live::start(&json);
    live.capture(4_800);
    let before = live.running.master_meter().left_peak;
    live.running.stop();

    offline(&json, 4_800, 512);

    let live = Live::start(&json);
    live.capture(4_800);
    let after = live.running.master_meter().left_peak;
    live.running.stop();
    assert!(before > 3.5, "{before}");
    assert_eq!(before.to_bits(), after.to_bits());
}
