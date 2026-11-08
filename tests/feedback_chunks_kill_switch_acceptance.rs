//! Acceptance tests for the kill switch of block-wise feedback
//! (`fugue::set_feedback_chunks`, `FUGUE_FEEDBACK_CHUNKS`). The flag is
//! process-global and read when a topology compiles, so every test that
//! toggles it lives in this binary, in one serialized `#[test]`; the
//! environment variable is checked in child processes.
//!
//! With the flag off every feedback group runs per sample (a loop is its delay
//! plus 1 sample per pass); on, the loop is exact; and every fallback case
//! renders the same, bit for bit, in either state.

#[path = "feedback_chunks_acceptance/rig.rs"]
mod rig;

use rig::*;
use serde_json::json;

const FRAMES: usize = 90_000;

fn loop_doc() -> Doc {
    gate_loop(delay_cfg(0.0625, Some(0.01)), 0.7, 30.0)
}

fn echoes(doc: &Doc) -> Vec<usize> {
    let (left, right) = offline(doc, FRAMES, 64);
    echo_offsets(&left, &right, 0, FRAMES)
}

fn assert_loop(want: Vec<usize>, label: &str) {
    assert_eq!(echoes(&loop_doc())[..6], want[..], "{label}");
}

fn ping_pong_gap() -> usize {
    let doc = gated(10.0)
        .add("a", "delay", json!({ "time": 0.375, "feedback": 0.0 }))
        .add("b", "delay", json!({ "time": 0.375, "feedback": 0.0 }))
        .add("ab", "vca", json!({ "level": 0.8 }))
        .add("ba", "vca", json!({ "level": 0.8 }))
        .wire("src", "audio", "a", "audio")
        .wire("a", "audio", "ab", "audio")
        .wire("ab", "audio", "b", "audio")
        .wire("b", "audio", "ba", "audio")
        .wire("ba", "audio", "a", "audio")
        .wire("a", "audio", "dac", "audio_left")
        .wire("b", "audio", "dac", "audio_right");
    let (left, right) = offline(&doc, 120_000, 64);
    let mut edges = rising_edges(&left, EDGE);
    edges.extend(rising_edges(&right, EDGE));
    edges.sort_unstable();
    // Gate, A, B, A: the second A repeat is one whole loop after the first.
    edges[3] - edges[1]
}

/// Every fallback case, as documents whose render must not depend on the flag.
fn fallback_docs() -> Vec<(&'static str, Doc, usize)> {
    let noise_loop = |patched: bool| {
        let cfg =
            json!({ "time": 0.0625, "min_time": 0.01, "feedback": 0.0, "time_mod_depth": 0.002 });
        let doc = Doc::new()
            .add("n", "noise", json!({ "seed": 5, "level": 0.2 }))
            .add("d", "delay", cfg)
            .add("loop", "vca", json!({ "level": 0.5 }))
            .add("m", "vca", json!({ "level": 1.0 }))
            .add("dac", "dac", json!({ "soft_clip": false }))
            .wire("n", "white", "d", "audio")
            .wire("d", "audio", "loop", "audio")
            .wire("loop", "audio", "d", "audio")
            .wire("d", "audio", "m", "audio")
            .wire("d", "audio", "dac", "audio_left")
            .wire("m", "audio", "dac", "audio_right");
        if patched {
            patch_mod(doc)
        } else {
            doc
        }
    };
    let side = loop_doc()
        .add("side", "vca", json!({ "level": 0.01 }))
        .wire("loop", "audio", "side", "audio")
        .wire("side", "audio", "loop", "audio");
    let zero = Doc::new()
        .add("n", "noise", json!({ "seed": 9, "level": 0.2 }))
        .add("a", "vca", json!({ "level": 0.6 }))
        .add("b", "vca", json!({ "level": 0.6 }))
        .add("dac", "dac", json!({ "soft_clip": false }))
        .wire("n", "white", "a", "audio")
        .wire("a", "audio", "b", "audio")
        .wire("b", "audio", "a", "audio")
        .wire("a", "audio", "dac", "audio_left")
        .wire("b", "audio", "dac", "audio_right");
    vec![
        ("time_mod fed from inside", noise_loop(true), 30_000),
        ("zero-delay cycle beside the loop", side, FRAMES),
        ("no delay in the cycle", zero, 20_000),
        (
            "time_mod patched loop with a gate",
            patch_mod(with_mod_tap(loop_doc())),
            FRAMES,
        ),
    ]
}

/// The kill switch: off, loops are the delay + 1 sample per pass; on, exact.
/// Everything the scheduler must fall back on renders identically either way.
#[test]
fn the_kill_switch_restores_per_sample_scheduling() {
    fugue::set_feedback_chunks(true);
    assert!(fugue::feedback_chunks());
    assert_loop(exact(3000, 6), "on");
    let on_gap = ping_pong_gap();
    let on_renders: Vec<_> = fallback_docs()
        .iter()
        .map(|(_, d, n)| offline(d, *n, 64))
        .collect();

    fugue::set_feedback_chunks(false);
    assert!(!fugue::feedback_chunks());
    assert_loop(per_sample(3000, 6), "off");
    let (on_gap, off_gap) = (on_gap, ping_pong_gap());
    assert_eq!(
        (on_gap, off_gap),
        (36_000, 36_001),
        "ping-pong loop, on and off"
    );
    for ((label, doc, frames), on) in fallback_docs().iter().zip(&on_renders) {
        let off = offline(doc, *frames, 64);
        assert_bits(&off.0, &on.0, &format!("{label}: left, off vs on"));
        assert_bits(&off.1, &on.1, &format!("{label}: right, off vs on"));
    }

    // Inside a development, too.
    let inside = loop_inside(delay_cfg(0.0625, Some(0.01)));
    let (left, right) = offline(&inside, FRAMES, 64);
    assert_eq!(
        echo_offsets(&left, &right, 0, FRAMES)[..6],
        per_sample(3000, 6)[..],
        "development, off"
    );
    fugue::set_feedback_chunks(true);
    let (left, right) = offline(&inside, FRAMES, 64);
    assert_eq!(
        echo_offsets(&left, &right, 0, FRAMES)[..6],
        exact(3000, 6)[..],
        "development, on"
    );

    // The flag is read when a topology compiles: a loaded invention keeps the
    // scheduling it was compiled with until an edit recompiles it.
    fugue::set_feedback_chunks(false);
    let mut loaded = engine(&gate_loop(delay_cfg(0.0625, Some(0.01)), 0.7, 60.0));
    fugue::set_feedback_chunks(true);
    let (left, right) = render(&mut loaded, 36_864);
    assert_eq!(
        echo_offsets(&left, &right, 0, left.len())[..6],
        per_sample(3000, 6)[..],
        "loaded off"
    );
    loaded
        .add_module("spare", "vca", &json!({}))
        .expect("a module is added");
    let (left, right) = render(&mut loaded, 48_000);
    assert_eq!(
        echo_offsets(&left, &right, 0, left.len())[..6],
        exact(3000, 6)[..],
        "after an edit"
    );
}

/// `FUGUE_FEEDBACK_CHUNKS` (0, off, false) turns the cut off at process start
/// and `set_feedback_chunks` overrides it; unset, it is on.
#[test]
fn the_environment_variable_turns_the_cut_off_at_process_start() {
    let exe = std::env::current_exe().unwrap();
    for value in [Some("0"), Some("off"), Some("false"), None] {
        let mut child = std::process::Command::new(&exe);
        child.args([
            "environment_child",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ]);
        child
            .env("KILL_SWITCH_CHILD", "1")
            .env_remove("FUGUE_FEEDBACK_CHUNKS");
        if let Some(value) = value {
            child.env("FUGUE_FEEDBACK_CHUNKS", value);
        }
        let out = child.output().expect("the child runs");
        let text = String::from_utf8_lossy(&out.stdout).to_string()
            + &String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "FUGUE_FEEDBACK_CHUNKS={value:?}: child failed:\n{text}"
        );
        assert!(text.contains("1 passed"), "the child ran its test:\n{text}");
    }
}

/// Runs only as a child of the test above.
#[test]
fn environment_child() {
    if std::env::var("KILL_SWITCH_CHILD").is_err() {
        return;
    }
    let off = matches!(
        std::env::var("FUGUE_FEEDBACK_CHUNKS").as_deref(),
        Ok("0" | "off" | "false")
    );
    assert_eq!(fugue::feedback_chunks(), !off, "the flag at process start");
    let want = if off {
        per_sample(3000, 6)
    } else {
        exact(3000, 6)
    };
    assert_loop(want, "from the environment");
    fugue::set_feedback_chunks(true);
    assert_loop(
        exact(3000, 6),
        "set_feedback_chunks(true) overrides the environment",
    );
    fugue::set_feedback_chunks(false);
    assert_loop(
        per_sample(3000, 6),
        "set_feedback_chunks(false) overrides the environment",
    );
}
