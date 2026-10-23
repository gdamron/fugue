//! A live session with timed structural edits, exported and replayed
//! offline: each edit that landed lands again on the same sample, so the
//! render matches what played sample for sample, and one refused live is
//! left out and counts as no commit.

use crate::control_request::{RtValue, When};
use crate::invention::manual_backend::{start_manual, SAMPLE_RATE};
use crate::invention::publish::EditTime;
use crate::rpc::{EditWhen, StructuralEdit};
use crate::{Invention, InventionBuilder, LoggedEditOutcome, LoggedWhen, RenderEngine, RequestLog};

const DOC: &str = include_str!("../../../../examples/step_sequencer.json");

/// The device's callback size, which is also the graph's block size.
const BLOCK: usize = 64;

/// Mid-block, so the landing splits a block.
const SWAP_AT: u64 = 20_011;

/// Far past the session, where its ttl has long run out.
const EXPIRES_AT: u64 = 200_003;

/// What a session sent the DAC, interleaved.
type Output = Vec<f32>;

/// A live session of [`DOC`] on a manual backend: the oscillator swapped
/// for a square one mid-block, a write to the new instance after it, an
/// edit batch on the next bar adding a second oscillator in its place, and
/// a swap whose ttl runs out before it is due. Returns the exported log,
/// through JSON, and what played.
fn live_session() -> (RequestLog, Output) {
    let invention = Invention::from_json(DOC).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(invention).unwrap();
    let (mut running, pump) = start_manual(runtime);
    let mut out = Output::new();
    let mut play = |blocks| {
        let (left, right) = pump.render_stereo(blocks);
        for (l, r) in left.into_iter().zip(right) {
            out.extend([l, r]);
        }
    };

    play(100);
    let square = serde_json::json!({ "waveform": "square" });
    let at = EditTime::AtSample(SWAP_AT);
    let swap =
        running
            .live
            .swap_module_at(SAMPLE_RATE, "osc", "oscillator", &square, true, at, None);
    swap.unwrap();
    play(300);
    // Resolved against the oscillator the swap built.
    running.live.submit_logged(
        ("osc", "waveform"),
        RtValue::U32(2),
        When::AtSample(30_007),
        None,
    );
    play(200);
    let connection = |from: &str, from_port: &str, to: &str, to_port: &str| {
        (
            from.to_string(),
            from_port.to_string(),
            to.to_string(),
            to_port.to_string(),
        )
    };
    let connect = |(from, from_port, to, to_port)| StructuralEdit::Connect {
        from,
        from_port,
        to,
        to_port,
    };
    let disconnect = |(from, from_port, to, to_port)| StructuralEdit::Disconnect {
        from,
        from_port,
        to,
        to_port,
    };
    let batch = [
        StructuralEdit::AddModule {
            id: "osc2".into(),
            module_type: "oscillator".into(),
            config: serde_json::json!({ "waveform": "triangle" }),
        },
        disconnect(connection("osc", "audio", "vca", "audio")),
        connect(connection("seq", "frequency", "osc2", "frequency")),
        connect(connection("osc2", "audio", "vca", "audio")),
    ];
    let bar = EditWhen::NextBeat {
        every: 4.0,
        offset: 0.0,
        clock: None,
    };
    running.apply_edits_at(&batch, &bar, None).unwrap();
    play(1000);
    let sine = serde_json::json!({ "waveform": "sine" });
    let at = EditTime::AtSample(EXPIRES_AT);
    let late =
        running
            .live
            .swap_module_at(SAMPLE_RATE, "osc2", "oscillator", &sine, true, at, Some(64));
    late.unwrap();
    play(1600);

    let log = running.request_log();
    let json = serde_json::to_string(&log).unwrap();
    running.stop();
    (serde_json::from_str(&json).unwrap(), out)
}

/// An offline render of [`DOC`] as loaded, in device-sized calls, replaying
/// `log` when there is one.
fn offline(log: Option<&RequestLog>, frames: usize) -> Output {
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_json(DOC).unwrap();
    if let Some(log) = log {
        engine.replay(log).unwrap();
    }
    let mut out = Output::with_capacity(frames * 2);
    let mut buffer = [0.0f32; BLOCK * 2];
    for _ in 0..frames / BLOCK {
        engine.render_interleaved(&mut buffer).unwrap();
        out.extend_from_slice(&buffer);
    }
    assert_eq!(engine.replay_refused(), 0);
    out
}

fn first_difference(a: &[f32], b: &[f32]) -> Option<usize> {
    let same = |(a, b): (&f32, &f32)| a.to_bits() == b.to_bits();
    a.iter()
        .zip(b)
        .position(|pair| !same(pair))
        .map(|at| at / 2)
}

#[test]
fn a_replayed_session_lands_its_timed_edits_on_the_same_samples() {
    let (log, played) = live_session();
    assert!(log.is_exact(), "{log:#?}");
    assert_eq!(log.edits, 0, "a timed edit is not counted as a commit");
    let outcomes: Vec<_> = log.timed_edits.iter().map(|edit| &edit.outcome).collect();
    let [swap, bar, late] = outcomes[..] else {
        panic!("{outcomes:?}");
    };
    assert_eq!(
        *swap,
        LoggedEditOutcome::Landed {
            sample: SWAP_AT,
            due: None
        }
    );
    let LoggedEditOutcome::Landed { sample: bar_at, .. } = *bar else {
        panic!("{bar:?}");
    };
    assert!(matches!(log.timed_edits[1].when, LoggedWhen::Beat { .. }));
    assert_eq!(
        *late,
        LoggedEditOutcome::Refused {
            reason: "expired".into()
        }
    );
    assert_eq!(log.timed_edits[1].built[0].id, "osc2");

    let frames = played.len() / 2;
    let replayed = offline(Some(&log), frames);
    assert_eq!(
        first_difference(&played, &replayed),
        None,
        "the edit landed on {bar_at}"
    );
    // The edits are heard: without them the render parts at the swap.
    let loaded = offline(None, frames);
    assert_eq!(first_difference(&played, &loaded), Some(SWAP_AT as usize));
}
