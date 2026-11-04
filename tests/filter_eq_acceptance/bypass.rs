//! "A bypassed band is bit-identical to its input; toggling bypass
//! crossfades over at least 5 ms."

use crate::motion::{assert_no_click, assert_spread};
use crate::{filter, max_diff, noise, sine, AMP, MS, SR};
use serde_json::json;

fn every_mode() -> Vec<serde_json::Value> {
    let mut configs = vec![
        json!({ "filter_type": "bell", "cutoff": 800.0, "q": 4.0, "boost": 9.0 }),
        json!({ "filter_type": "lowshelf", "cutoff": 300.0, "boost": -10.0 }),
        json!({ "filter_type": "highshelf", "cutoff": 3000.0, "boost": 6.0 }),
        json!({ "filter_type": "notch", "cutoff": 1000.0, "q": 2.0 }),
    ];
    for mode in ["lowpass", "highpass", "bandpass"] {
        for slope in ["12db", "24db"] {
            configs.push(
                json!({ "filter_type": mode, "cutoff": 900.0, "resonance": 0.8, "slope": slope }),
            );
        }
    }
    configs
}

/// Bypass set before the filter is heard (in config, or written before the
/// first block) lands at once: the output is the input, sample for sample.
#[test]
fn bypassed_filter_is_bit_identical_to_its_input() {
    let input = noise(SR as usize / 4, AMP, 0xb1a5);
    for mut config in every_mode() {
        let mut written = filter(config.clone());
        written.set("bypass", 1.0);
        let out = written.run(&input, &[]);
        assert!(
            out == input,
            "{config}: bypass written before audio passes input through"
        );
        config["bypass"] = json!(true);
        let out = filter(config.clone()).run(&input, &[]);
        assert!(out == input, "{config}: output is the input");
    }
}

/// Live toggles on a running +12 dB bell fed a 997 Hz tone: switching bypass
/// on moves from the filtered to the dry signal without a click, spread over
/// at least 5 ms, and is bit-identical to the input once the crossfade is
/// over (15 ms on: the pinned 10 ms crossfade plus block slack). Switching
/// it off again moves back to the filtered signal the same way.
#[test]
fn toggling_bypass_crossfades() {
    let input = sine(SR as usize, 997.0, AMP);
    let config = json!({ "filter_type": "bell", "cutoff": 1000.0, "q": 2.0, "boost": 12.0 });
    let filtered = filter(config.clone()).run(&input, &[]);
    let (on, off) = (SR as usize / 4 + 7, SR as usize / 2 + 31);
    let out = filter(config).run(&input, &[(on, "bypass", 1.0), (off, "bypass", 0.0)]);

    assert_no_click("bypass toggles", &out, &[&filtered, &input]);
    assert_spread("bypass on", &out, &filtered, &input, on, 5);
    let early = on..on + 5 * MS;
    let reached = (early.clone())
        .find(|&n| (filtered[n] - input[n]).abs() > 0.01 && out[n].to_bits() == input[n].to_bits());
    assert!(
        reached.is_none(),
        "bypass on: already dry at {:?}",
        reached.map(|n| n - on)
    );
    let settled = on + 15 * MS..off;
    assert!(
        out[settled.clone()] == input[settled],
        "bypass on: bit-identical to the input once the crossfade is over"
    );

    assert_spread("bypass off", &out, &input, &filtered, off, 5);
    let late = off + 50 * MS..;
    let miss = max_diff(&out[late.clone()], &filtered[late]);
    assert!(
        miss < 1e-3,
        "bypass off: back to filtered within 50 ms ({miss})"
    );
}
