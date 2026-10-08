use super::*;
use crate::invention::builder::InventionBuilder;
use crate::invention::format::Invention;
use crate::invention::orchestration::OrchestrationRuntime;
use crate::modules::AudioDiagnostics;
use crate::test_support::wait_until;
use crate::ControlValue;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

mod editing;
mod non_finite;
#[cfg(feature = "spectrogram")]
mod spectrum;

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

/// Waits for the running graph to list `module_id`. Code module hooks run on
/// a script thread, which a loaded machine can leave waiting well past any
/// fixed sleep.
fn wait_for_module(running: &RunningInvention, module_id: &str) -> bool {
    wait_until(|| {
        running
            .list_modules()
            .into_iter()
            .any(|module| module.id == module_id)
    })
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
                "script": "function init() { graph.addModule('osc_live', 'oscillator', { type: 'sine', frequency: 220.0 }) }"
            }),
        )
        .unwrap();

    // The audio worker and the code module's script thread both run on their
    // own schedule; wait for each rather than sleeping a fixed time.
    assert!(wait_until(|| running
        .status()
        .diagnostics
        .as_ref()
        .is_some_and(|diagnostics| diagnostics.callback_count > 0)));
    assert!(running
        .full_snapshot()
        .status
        .diagnostics
        .as_ref()
        .is_some_and(|diagnostics| diagnostics.callback_count > 0));

    assert!(wait_for_module(&running, "osc_live"));

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

    let ticked = ControlValue::String("tick-ran".to_string());
    wait_until(|| running.get_control("code1", "last_error").unwrap() == ticked);
    assert_eq!(running.get_control("code1", "last_error").unwrap(), ticked);

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
                        "script": "(() => ({ init() { graph.addModule('osc_from_object_live', 'oscillator', { type: 'sine', frequency: 330.0 }) } }))()"
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

    assert!(wait_for_module(&running, "osc_from_object_live"));

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
                { "id": "osc", "type": "oscillator", "config": { "type": "sine", "frequency": 440.0 } },
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
            crate::ControlWrite::new(
                "osc".to_string(),
                "frequency".to_string(),
                ControlValue::Number(880.0),
            ),
            crate::ControlWrite::new(
                "osc".to_string(),
                "type".to_string(),
                ControlValue::String("square".to_string()),
            ),
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

    // Let ticks run until the script's console.log fires with the sink in place.
    let logged = |activities: &[(String, String)]| {
        activities.iter().any(|(module_id, activity)| {
            module_id == "code1" && activity == "[log] conducting section B"
        })
    };
    wait_until(|| logged(&sink.agent_activities()));
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
                { "id": "osc", "type": "oscillator", "config": { "type": "sine", "frequency": 440.0 } },
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

    // Let the audio worker render enough blocks to fold in a peak. Reading
    // the meter drains it, so keep the reading that satisfied the wait.
    let mut peaks = (0.0, 0.0);
    wait_until(|| {
        peaks = running.master_meter();
        peaks.0 > 0.0 && peaks.1 > 0.0
    });

    let (left, right) = peaks;
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
                        "script": "globalThis.init = function () { graph.addModule('osc_from_legacy_live', 'oscillator', { type: 'sine', frequency: 260.0 }) }"
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

    assert!(wait_for_module(&running, "osc_from_legacy_live"));

    running.stop();
}

#[test]
fn performed_control_writes_are_announced_but_never_authored() {
    let invention = Invention::from_json(
        r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "osc", "type": "oscillator", "config": { "type": "sine", "frequency": 440.0 } },
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

    let authored_frequency = |running: &RunningInvention| {
        running
            .document()
            .unwrap()
            .modules
            .iter()
            .find(|spec| spec.id == "osc")
            .and_then(|spec| spec.config["frequency"].as_f64())
    };

    // A performed gesture (a scheduler, a script, a live knob) is applied and
    // announced, but the authored starting state is untouched (FUG-266).
    running
        .snapshot()
        .set_control_with_intent(
            "osc",
            "frequency",
            ControlValue::String("660".to_string()),
            crate::ControlWriteIntent::Perform,
        )
        .unwrap();
    assert_eq!(
        running.get_control("osc", "frequency").unwrap(),
        ControlValue::Number(660.0),
        "perform still coerces and applies"
    );
    assert_eq!(authored_frequency(&running), Some(440.0));

    // An authoring write sets the new starting state.
    running
        .set_controls(&[crate::ControlWrite::new(
            "osc",
            "frequency",
            ControlValue::Number(550.0),
        )])
        .unwrap();
    assert_eq!(authored_frequency(&running), Some(550.0));

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
                ControlValue::Number(550.0)
            ),
        ],
        "both intents are observable as control changes"
    );
}
