//! Criteria 3 and 7: repeat spacing with feedback inside and outside the
//! atom, and stability at feedback 1.1.

use super::rig::*;
use super::*;

/// Criterion 3: a loop patched outside the atom (delay.audio -> vca -> back
/// to delay.audio) also spaces repeats exactly the delay time. A short clock
/// gate, scaled to 0.5, feeds the delay; the dry gate's rising edge and each
/// echo's are compared (the first echo is 12,000 after the dry edge, then
/// 12,000 apart).
#[test]
fn outside_feedback_loop_spaces_repeats_exactly_time() {
    let doc = json!({
        "version": "1.0.0",
        "modules": [
            { "id": "clk", "type": "clock", "config": { "bpm": 30.0, "gate_length": 0.001 } },
            { "id": "src", "type": "vca", "config": { "level": 0.5 } },
            { "id": "d", "type": "delay", "config": { "time": 0.25, "feedback": 0.0 } },
            { "id": "loop", "type": "vca", "config": { "level": 0.5 } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            conn("clk", "beat", "src", "audio"),
            conn("src", "audio", "dac", "audio_left"),
            conn("src", "audio", "d", "audio"),
            conn("d", "audio", "loop", "audio"),
            conn("loop", "audio", "d", "audio"),
            conn("d", "audio", "dac", "audio_right")
        ]
    });
    let (dry, wet) = render(&mut engine(doc), 96_000);
    let start = rising_edges(&dry, 0.02)[0];
    let echoes = rising_edges(&wet, 0.02);
    assert!(echoes.len() >= 5, "five echoes heard, edges at {echoes:?}");
    let want: Vec<usize> = (0..5).map(|k| start + 12_000 * (k + 1)).collect();
    assert_eq!(echoes[..5], want[..], "edge of the dry gate at {start}");
}

/// Criterion 7: feedback 1.1 with 0 dBFS uniform noise stays within +/-1.0
/// for 60 s. Assumption: the input is sustained noise, so the loop saturates
/// at the soft clip rather than decaying.
#[test]
fn feedback_of_1_1_with_full_scale_noise_stays_within_unity_for_60_seconds() {
    let seconds = 60;
    let mut d = delay(json!({ "time": 0.1, "feedback": 1.1 }));
    let mut worst = 0.0f32;
    for second in 0..seconds {
        let out = d.render(&noise(SR as usize, 1.0, 1 + second));
        assert!(
            out.iter().all(|v| v.is_finite()),
            "finite in second {second}"
        );
        worst = worst.max(peak(&out));
    }
    assert!(worst <= 1.0, "peak {worst} left +/-1.0");
    assert!(worst > 0.5, "the saturated loop is audible (peak {worst})");
}

/// "so the output never leaves +/-1, at any feedback, with an outside loop
/// too": full-scale noise into a unity-gain outside loop, tapped through a
/// 0.5 level so the dac's own clamp cannot hide an overshoot.
#[test]
fn outside_loop_at_unity_gain_stays_within_unity() {
    let doc = json!({
        "version": "1.0.0",
        "modules": [
            { "id": "src", "type": "noise", "config": { "seed": 3, "level": 1.0 } },
            { "id": "d", "type": "delay", "config": { "time": 0.05, "feedback": 1.1 } },
            { "id": "loop", "type": "vca", "config": { "level": 1.0 } },
            { "id": "tap", "type": "vca", "config": { "level": 0.5 } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            conn("src", "white", "d", "audio"),
            conn("d", "audio", "loop", "audio"),
            conn("loop", "audio", "d", "audio"),
            conn("d", "audio", "tap", "audio"),
            conn("tap", "audio", "dac", "audio_left")
        ]
    });
    let mut engine = engine(doc);
    for _ in 0..10 {
        let (left, _) = render(&mut engine, 48_000);
        assert!(
            peak(&left) <= 0.5 + 1e-6,
            "delay output left +/-1: tap peak {}",
            peak(&left)
        );
    }
}
