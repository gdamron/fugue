//! A replayed timed edit that builds a module of a development the
//! invention declares: the render loads the invention's developments, so
//! the replay builds it as live did and plays what played.

use crate::invention::manual_backend::{start_manual, SAMPLE_RATE};
use crate::rpc::{EditWhen, StructuralEdit};
use crate::{Invention, InventionBuilder, RenderEngine, RequestLog};

const BLOCK: usize = 64;

/// The step sequencer's line, with a `voice` development declared and
/// not yet used.
fn document() -> String {
    let mut doc: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/step_sequencer.json")).unwrap();
    doc["developments"] = serde_json::json!([{
        "name": "voice",
        "definition": {
            "version": "1.0.0",
            "modules": [
                { "id": "osc", "type": "oscillator", "config": { "waveform": "square" } },
                { "id": "vca", "type": "vca" }
            ],
            "connections": [
                { "from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio" }
            ],
            "inputs": [
                { "name": "frequency", "to": "osc", "to_port": "frequency" },
                { "name": "level", "to": "vca", "to_port": "level" }
            ],
            "outputs": [{ "name": "audio", "from": "vca", "from_port": "audio" }],
            "controls": [{ "name": "waveform", "module": "osc", "control": "waveform" }]
        }
    }]);
    doc.to_string()
}

fn connect(from: &str, from_port: &str, to: &str, to_port: &str) -> StructuralEdit {
    StructuralEdit::Connect {
        from: from.into(),
        from_port: from_port.into(),
        to: to.into(),
        to_port: to_port.into(),
    }
}

#[test]
fn a_replayed_edit_builds_a_development_the_invention_declares() {
    let doc = document();
    let invention = Invention::from_json(&doc).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(invention).unwrap();
    let (mut running, pump) = start_manual(runtime);
    let mut played = Vec::new();
    let mut play = |blocks| {
        let (left, right) = pump.render_stereo(blocks);
        for (l, r) in left.into_iter().zip(right) {
            played.extend([l, r]);
        }
    };
    play(200);
    // On the next bar, a voice joins the line on the right.
    let batch = [
        StructuralEdit::AddModule {
            id: "voice2".into(),
            module_type: "voice".into(),
            config: serde_json::json!({}),
        },
        connect("seq", "frequency", "voice2", "frequency"),
        connect("adsr", "envelope", "voice2", "level"),
        connect("voice2", "audio", "dac", "audio_right"),
    ];
    let bar = EditWhen::NextBeat {
        every: 4.0,
        offset: 0.0,
        clock: None,
    };
    running.apply_edits_at(&batch, &bar, None).unwrap();
    play(2_000);
    let json = serde_json::to_string(&running.request_log()).unwrap();
    running.stop();
    let log: RequestLog = serde_json::from_str(&json).unwrap();
    assert!(log.is_exact(), "{log:#?}");
    assert_eq!(log.timed_edits[0].built[0].module_type, "voice");
    let landed = log.timed_edits[0].landed().expect("it landed");

    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_json(&doc).unwrap();
    engine.replay(&log).unwrap();
    let mut replayed = vec![0.0f32; played.len()];
    for chunk in replayed.chunks_mut(BLOCK * 2) {
        engine.render_interleaved(chunk).unwrap();
    }
    assert_eq!(engine.replay_refused(), 0);
    let same = |(a, b): (&f32, &f32)| a.to_bits() == b.to_bits();
    let first = played.iter().zip(&replayed).position(|pair| !same(pair));
    assert_eq!(first, None, "the voice landed on {landed}");
    // The voice is heard on the right after it lands, and not before.
    let right = |at: usize| played[at * 2 + 1] != played[at * 2];
    assert!(!(0..landed as usize).any(right));
    assert!((landed as usize..played.len() / 2).any(right));
}
