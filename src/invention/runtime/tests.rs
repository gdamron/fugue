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
