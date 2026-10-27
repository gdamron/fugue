//! A conducting script's timed control writes: `setControl` takes the
//! wire's `{ when, ttl_ms }` as a fourth argument and returns the write's
//! request, so a later write can follow it; a time that cannot be kept, or
//! a misspelt one, throws where the script can catch it.

use super::*;

const SCRIPT: &str = r#"
let done = false;
function tick() {
    if (done) return;
    done = true;
    const said = [];
    const first = graph.setControl('osc', 'frequency', 330,
        { when: { kind: 'after_samples', samples: 2400 } });
    said.push(first);
    graph.setControl('osc', 'frequency', 440,
        { when: { kind: 'after_request', request: first, offset_samples: 2400 }, ttl_ms: 60000 });
    try {
        graph.setControl('code1', 'last_error', 'x', { when: { kind: 'after_samples', samples: 1 } });
    } catch (error) { said.push(String(error)); }
    try {
        graph.setControl('osc', 'frequency', 1, { wen: { kind: 'after_samples', samples: 1 } });
    } catch (error) { said.push(String(error)); }
    graph.setControl('code1', 'last_error', said.join(' | '));
}
"#;

#[test]
fn a_script_times_a_write_after_its_own_and_hears_why_one_cannot_wait() {
    let document = serde_json::json!({
        "version": "1.0.0",
        "modules": [
            { "id": "osc", "type": "oscillator", "config": { "frequency": 220.0 } },
            { "id": "code1", "type": "code", "config": { "tick_rate": 20.0, "script": SCRIPT } },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [
            { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]
    });
    let invention = Invention::from_json(&document.to_string()).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let running = runtime
        .start_with_backend(TickBackend::new(48_000))
        .unwrap();

    let said = || match running.get_control("code1", "last_error").unwrap() {
        ControlValue::String(said) => said,
        other => panic!("{other:?}"),
    };
    assert!(wait_until(|| said().contains(" | ")), "{}", said());
    let said = said();
    let parts: Vec<_> = said.split(" | ").collect();
    assert!(parts[0].parse::<u64>().is_ok(), "a request id: {said}");
    assert!(parts[1].contains("cannot be timed"), "{said}");
    assert!(parts[2].contains("`wen`"), "{said}");

    let heard = ControlValue::Number(440.0);
    assert!(wait_until(|| running
        .get_control("osc", "frequency")
        .unwrap()
        == heard));
    running.stop();
}
