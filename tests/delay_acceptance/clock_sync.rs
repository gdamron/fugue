//! Criterion 5 and the `clock` module's new `bpm` output.

use super::rig::*;
use super::*;

fn clock(config: Value) -> GraphModule {
    ModuleRegistry::default()
        .build("clock", SR, &config)
        .expect("clock builds")
        .module
}

fn render_clock(clk: &mut GraphModule, frames: usize) -> Vec<Vec<f32>> {
    let m = clk.module_mut();
    let mut out = vec![Vec::new(); m.outputs().len()];
    for _ in 0..frames / BLOCK {
        assert!(m.process(BLOCK));
        for (port, channel) in out.iter_mut().enumerate() {
            channel.extend_from_slice(&m.output_block(port)[..BLOCK]);
        }
    }
    out
}

/// "existing clock outputs unchanged": at 120 bpm the gates rise every 24,000
/// (beat), 12,000 (x2), 6,000 (x4) and 48,000 (d2) samples; a beat gate is
/// high for its duty cycle of 0.25 beat (6,000 samples).
#[test]
fn existing_gate_outputs_keep_their_timing() {
    let out = render_clock(&mut clock(json!({ "bpm": 120.0 })), 1_200 * BLOCK);
    for (port, period) in [(0, 24_000), (3, 12_000), (4, 6_000), (2, 48_000)] {
        let edges = rising_edges(&out[port], 0.5);
        assert!(edges.len() >= 3, "port {port} rises");
        // The first gate rises on the clock's first sample; later gates are
        // exactly one period apart.
        assert!(
            edges[1..].windows(2).all(|w| w[1] - w[0] == period),
            "port {port}: {edges:?}"
        );
    }
    let high = out[0].iter().take(24_000).filter(|&&v| v > 0.5).count();
    assert_eq!(high, 6_000);
}

/// In an invention, `bpm` follows a tempo change on the next block. It is
/// scaled by a 0.001 level so the dac's clamp does not hide it.
#[test]
fn bpm_output_follows_a_tempo_change_in_an_invention() {
    let doc = json!({
        "version": "1.0.0",
        "modules": [
            { "id": "tempo", "type": "clock", "config": { "bpm": 90.0 } },
            { "id": "scale", "type": "vca", "config": { "level": 0.001 } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            conn("tempo", "bpm", "scale", "audio"),
            conn("scale", "audio", "dac", "audio_left")
        ]
    });
    let mut engine = engine(doc);
    let (before, _) = render(&mut engine, 48_000);
    set(&engine, "tempo", "bpm", 140.0f32);
    let (after, _) = render(&mut engine, 48_000);
    assert!(
        before[4_800..].iter().all(|v| (v - 0.09).abs() < 1e-4),
        "90 bpm"
    );
    assert!(
        after[BLOCK..].iter().all(|v| (v - 0.14).abs() < 1e-4),
        "140 bpm"
    );
}

/// Criterion 5: a delay (dotted 1/8) patched from `clock.bpm` follows a clock
/// tempo change from 90 to 140 bpm. A gate every 2 s feeds the delay: the
/// echo of the first gate comes 0.75 beat = 24,000 samples at 90 bpm, the
/// echo of the second 15,428.57 samples at 140 bpm.
#[test]
fn echoes_follow_a_clock_tempo_change_from_90_to_140_bpm() {
    let doc = json!({
        "version": "1.0.0",
        "modules": [
            { "id": "clk", "type": "clock", "config": { "bpm": 30.0, "gate_length": 0.001 } },
            { "id": "gate", "type": "vca", "config": { "level": 0.5 } },
            { "id": "tempo", "type": "clock", "config": { "bpm": 90.0 } },
            { "id": "d", "type": "delay", "config": { "division": "dotted_8th_note" } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            conn("clk", "beat", "gate", "audio"),
            conn("gate", "audio", "dac", "audio_left"),
            conn("gate", "audio", "d", "audio"),
            conn("tempo", "bpm", "d", "bpm"),
            conn("d", "audio", "dac", "audio_right")
        ]
    });
    let mut engine = engine(doc);
    let (mut dry, mut wet) = render(&mut engine, 48_000);
    set(&engine, "tempo", "bpm", 140.0f32);
    let (l, r) = render(&mut engine, 72_000);
    dry.extend(l);
    wet.extend(r);
    let (gates, echoes) = (rising_edges(&dry, 0.2), rising_edges(&wet, 0.2));
    assert_eq!(
        (gates.len(), echoes.len()),
        (2, 2),
        "gates {gates:?} echoes {echoes:?}"
    );
    assert_eq!(echoes[0] - gates[0], 24_000, "echo at 90 bpm");
    let gap = (echoes[1] - gates[1]) as f64;
    assert!(
        (gap - 15_428.57).abs() <= 2.0,
        "echo at 140 bpm came {gap} samples after its gate"
    );
}
