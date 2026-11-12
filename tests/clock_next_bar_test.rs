//! A "next bar" write waiting on a clock whose bar lines move before it
//! lands: it follows them.

use std::sync::{Arc, Mutex};

use fugue::rpc::{ControlTiming, ControlWhen};
use fugue::{
    AudioBackend, ControlValue, ControlWrite, ControlWriteIntent, Invention, InventionBuilder,
    RunningInvention,
};
use serde_json::json;

const RATE: u32 = 48_000;

type Render = Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>;

struct Capture(Arc<Mutex<Option<Render>>>);

impl AudioBackend for Capture {
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn start(&mut self, render: Render) -> Result<(), Box<dyn std::error::Error>> {
        *self.0.lock().unwrap() = Some(render);
        Ok(())
    }
    fn stop(&mut self) {
        self.0.lock().unwrap().take();
    }
}

fn number(running: &RunningInvention, module: &str, key: &str) -> f32 {
    match running.get_control(module, key) {
        Ok(ControlValue::Number(n)) => n,
        other => panic!("{module}.{key}: {other:?}"),
    }
}

#[test]
fn a_waiting_next_bar_write_follows_a_meter_set_on_a_clock_without_one() {
    // 225 bpm at 48 kHz: a beat is 200 blocks of 64 frames.
    let invention: Invention = serde_json::from_value(json!({
        "version": "1.0.0",
        "modules": [
            { "id": "clock", "type": "clock", "config": { "bpm": 225.0 } },
            { "id": "osc", "type": "oscillator", "config": { "frequency": 220.0 } },
            { "id": "dac", "type": "dac" }
        ],
        "connections": [
            { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]
    }))
    .unwrap();
    let (runtime, _) = InventionBuilder::new(RATE).build(invention).unwrap();
    let slot = Arc::new(Mutex::new(None));
    let running = runtime.start_with_backend(Capture(slot.clone())).unwrap();
    let run_to = |beat: f32| {
        while number(&running, "clock", "position") < beat {
            let mut slot = slot.lock().unwrap();
            (slot.as_mut().unwrap())(&mut [0.0; 64], &mut [0.0; 64]);
        }
    };

    run_to(4.5);
    // No meter: the next bar is beat 8, 4 beats from beat 0.
    let write = ControlWrite {
        intent: ControlWriteIntent::Perform,
        timing: ControlTiming {
            when: Some(ControlWhen::NextBar {
                beats_per_bar: None,
                clock: None,
            }),
            ttl_ms: None,
        },
        ..ControlWrite::new("osc", "frequency", ControlValue::Number(330.0))
    };
    assert!(running.write_controls(&[write]).error.is_none());
    run_to(5.5);
    // Bars of 3 from beat 0 put the next line at 6, not 8.
    running
        .set_control("clock", "beats_per_bar", ControlValue::Number(3.0))
        .unwrap();
    run_to(5.9);
    assert_eq!(number(&running, "osc", "frequency"), 220.0);
    run_to(6.5);
    assert_eq!(number(&running, "osc", "frequency"), 330.0, "on beat 6");
}
