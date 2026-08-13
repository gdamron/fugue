use super::*;
use crate::invention::builder::InventionBuilder;
use crate::invention::format::Invention;
use crate::modules::AudioDiagnostics;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

struct TickBackend {
    sample_rate: u32,
    stop: Arc<AtomicBool>,
    diagnostics: Arc<AudioDiagnostics>,
    worker: Option<JoinHandle<()>>,
}

impl TickBackend {
    fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            stop: Arc::new(AtomicBool::new(false)),
            diagnostics: Arc::new(AudioDiagnostics::new()),
            worker: None,
        }
    }
}

impl AudioBackend for TickBackend {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(
        &mut self,
        mut render: Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let stop = self.stop.clone();
        let diagnostics = self.diagnostics.clone();
        self.worker = Some(thread::spawn(move || {
            let mut left = [0.0f32; 64];
            let mut right = [0.0f32; 64];
            while !stop.load(Ordering::Relaxed) {
                let started = std::time::Instant::now();
                render(&mut left, &mut right);
                let callback_ns = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
                diagnostics.record_callback(callback_ns, 1_333_333);
                thread::sleep(Duration::from_millis(2));
            }
        }));
        Ok(())
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    fn diagnostics(&self) -> Option<Arc<AudioDiagnostics>> {
        Some(self.diagnostics.clone())
    }
}

#[test]
fn running_invention_tracks_runtime_module_mutations() {
    let invention = Invention::from_json(
        r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "dac", "type": "dac" }
            ],
            "connections": []
        }"#,
    )
    .unwrap();

    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let running = runtime
        .start_with_backend(TickBackend::new(48_000))
        .unwrap();

    assert_eq!(running.list_modules().len(), 1);
    running
        .add_module(
            "code1",
            "code",
            &serde_json::json!({
                "script": "function init() { graph.addModule('osc_live', 'oscillator', { waveform: 'sine', frequency: 220.0 }) }"
            }),
        )
        .unwrap();

    thread::sleep(Duration::from_millis(50));

    let status = running.status();
    assert!(status
        .diagnostics
        .as_ref()
        .is_some_and(|diagnostics| diagnostics.callback_count > 0));
    assert!(running
        .full_snapshot()
        .status
        .diagnostics
        .as_ref()
        .is_some_and(|diagnostics| diagnostics.callback_count > 0));

    assert!(running
        .list_modules()
        .into_iter()
        .any(|module| module.id == "osc_live"));

    running.remove_module("osc_live").unwrap();
    assert!(!running
        .list_modules()
        .into_iter()
        .any(|module| module.id == "osc_live"));

    running.stop();
}

#[test]
fn running_invention_code_tick_updates_controls() {
    let invention = Invention::from_json(
        r#"{
            "version": "1.0.0",
            "modules": [
                {
                    "id": "code1",
                    "type": "code",
                    "config": {
                        "tick_hz": 20.0,
                        "script": "function tick() { graph.setControl('code1', 'last_error', 'tick-ran') }"
                    }
                },
                { "id": "dac", "type": "dac" }
            ],
            "connections": []
        }"#,
    )
    .unwrap();

    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let running = runtime
        .start_with_backend(TickBackend::new(48_000))
        .unwrap();

    thread::sleep(Duration::from_millis(120));

    assert_eq!(
        running.get_control("code1", "last_error").unwrap(),
        ControlValue::String("tick-ran".to_string())
    );

    running.stop();
}

#[test]
fn running_invention_supports_returned_lifecycle_object() {
    let invention = Invention::from_json(
        r#"{
            "version": "1.0.0",
            "modules": [
                {
                    "id": "code1",
                    "type": "code",
                    "config": {
                        "script": "(() => ({ init() { graph.addModule('osc_from_object_live', 'oscillator', { waveform: 'sine', frequency: 330.0 }) } }))()"
                    }
                },
                { "id": "dac", "type": "dac" }
            ],
            "connections": []
        }"#,
    )
    .unwrap();

    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let running = runtime
        .start_with_backend(TickBackend::new(48_000))
        .unwrap();

    thread::sleep(Duration::from_millis(50));

    assert!(running
        .list_modules()
        .into_iter()
        .any(|module| module.id == "osc_from_object_live"));

    running.stop();
}

/// Collects every event a runtime announces, for asserting emission.
#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<crate::RpcEventPayload>>,
}

impl crate::RpcEventSink for RecordingSink {
    fn emit(&self, event: crate::RpcEvent) {
        self.events.lock().unwrap().push(event.payload);
    }
}

impl RecordingSink {
    fn control_changes(&self) -> Vec<(String, String, ControlValue)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                crate::RpcEventPayload::ControlChanged {
                    module_id,
                    key,
                    value,
                } => Some((module_id.clone(), key.clone(), value.clone())),
                _ => None,
            })
            .collect()
    }

    fn agent_activities(&self) -> Vec<(String, String)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                crate::RpcEventPayload::AgentActivity {
                    module_id,
                    activity,
                } => Some((module_id.clone(), activity.clone())),
                _ => None,
            })
            .collect()
    }
}

#[test]
fn installed_sink_observes_control_writes_with_the_applied_value() {
    let invention = Invention::from_json(
        r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "osc", "type": "oscillator", "config": { "waveform": "sine", "frequency": 440.0 } },
                { "id": "dac", "type": "dac" }
            ],
            "connections": [
                { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
            ]
        }"#,
    )
    .unwrap();

    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let running = runtime
        .start_with_backend(TickBackend::new(48_000))
        .unwrap();

    let sink = Arc::new(RecordingSink::default());
    running.set_event_sink(sink.clone());

    // A stringified write coerces to the frequency control's Number kind; the
    // event must carry the *applied* value, not the raw string (FUG-239 #7).
    running
        .set_control("osc", "frequency", ControlValue::String("660".to_string()))
        .unwrap();
    // A telemetry-only transient write must not surface as a control change.
    running
        .snapshot()
        .set_control_transient("osc", "frequency", ControlValue::Number(770.0))
        .unwrap();
    // A batch lands one event per write.
    running
        .set_controls(&[
            crate::ControlWrite {
                module_id: "osc".to_string(),
                key: "frequency".to_string(),
                value: ControlValue::Number(880.0),
            },
            crate::ControlWrite {
                module_id: "osc".to_string(),
                key: "type".to_string(),
                value: ControlValue::String("square".to_string()),
            },
        ])
        .unwrap();

    running.stop();

    assert_eq!(
        sink.control_changes(),
        vec![
            (
                "osc".to_string(),
                "frequency".to_string(),
                ControlValue::Number(660.0)
            ),
            (
                "osc".to_string(),
                "frequency".to_string(),
                ControlValue::Number(880.0)
            ),
            (
                "osc".to_string(),
                "type".to_string(),
                ControlValue::String("square".to_string())
            ),
        ]
    );
}

#[test]
fn installed_sink_observes_code_module_console_output() {
    let invention = Invention::from_json(
        r#"{
            "version": "1.0.0",
            "modules": [
                {
                    "id": "code1",
                    "type": "code",
                    "config": {
                        "tick_hz": 20.0,
                        "script": "function tick() { console.log('conducting', 'section B') }"
                    }
                },
                { "id": "dac", "type": "dac" }
            ],
            "connections": []
        }"#,
    )
    .unwrap();

    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let running = runtime
        .start_with_backend(TickBackend::new(48_000))
        .unwrap();

    let sink = Arc::new(RecordingSink::default());
    running.set_event_sink(sink.clone());

    // Let a few ticks run so the script's console.log fires with the sink in place.
    thread::sleep(Duration::from_millis(120));
    running.stop();

    let activities = sink.agent_activities();
    assert!(
        activities
            .iter()
            .any(|(module_id, activity)| module_id == "code1"
                && activity == "[log] conducting section B"),
        "expected a code1 AgentActivity from console.log, got {activities:?}"
    );
}

#[test]
fn master_meter_reports_output_peaks() {
    let invention = Invention::from_json(
        r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "osc", "type": "oscillator", "config": { "waveform": "sine", "frequency": 440.0 } },
                { "id": "dac", "type": "dac" }
            ],
            "connections": [
                { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
            ]
        }"#,
    )
    .unwrap();

    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let running = runtime
        .start_with_backend(TickBackend::new(48_000))
        .unwrap();

    // Let the audio worker render enough blocks to fold in a peak.
    thread::sleep(Duration::from_millis(60));

    let (left, right) = running.master_meter();
    assert!(left > 0.0, "expected a non-zero left peak, got {left}");
    assert!(right > 0.0, "expected a non-zero right peak, got {right}");
    assert!(left <= 1.0 && right <= 1.0, "peaks stay within full-scale");

    running.stop();
}

#[test]
fn running_invention_keeps_legacy_globalthis_hooks_working() {
    let invention = Invention::from_json(
        r#"{
            "version": "1.0.0",
            "modules": [
                {
                    "id": "code1",
                    "type": "code",
                    "config": {
                        "script": "globalThis.init = function () { graph.addModule('osc_from_legacy_live', 'oscillator', { waveform: 'sine', frequency: 260.0 }) }"
                    }
                },
                { "id": "dac", "type": "dac" }
            ],
            "connections": []
        }"#,
    )
    .unwrap();

    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let running = runtime
        .start_with_backend(TickBackend::new(48_000))
        .unwrap();

    thread::sleep(Duration::from_millis(50));

    assert!(running
        .list_modules()
        .into_iter()
        .any(|module| module.id == "osc_from_legacy_live"));

    running.stop();
}
