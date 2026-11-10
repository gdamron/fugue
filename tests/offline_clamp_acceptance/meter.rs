//! Criterion: the peak meter still reports the pre-clamp peak, as it does
//! live. The offline engine has no meter, so this reads the live master
//! meter, headless through a capturing backend.

use super::support::*;

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
