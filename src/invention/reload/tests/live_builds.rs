//! A reload checks the new document without building a module live, and
//! never rebuilds a running recorder over the file it is writing.

use super::*;

/// Checking a reloaded document never builds a running recorder again over
/// the file it is writing.
#[test]
fn reload_never_rebuilds_a_running_recorder_over_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("take.wav");
    let with_recorder = |frequency: &str| {
        BASE.replace(
            r#"{ "id": "dac", "type": "dac" }"#,
            &format!(
                r#"{{ "id": "dac", "type": "dac" }},
                {{ "id": "rec", "type": "audio_file_sink", "config": {{ "path": {path:?} }} }}"#
            ),
        )
        .replace(
            r#"{ "from": "osc2", "from_port": "audio", "to": "dac", "to_port": "audio" }"#,
            r#"{ "from": "osc2", "from_port": "audio", "to": "dac", "to_port": "audio" },
            { "from": "osc1", "from_port": "audio", "to": "rec", "to_port": "audio" }"#,
        )
        .replace("440.0", frequency)
    };
    let (runtime, _) = InventionBuilder::new(crate::invention::manual_backend::SAMPLE_RATE)
        .build(doc(&with_recorder("440.0")))
        .unwrap();
    let (mut running, pump) = crate::invention::manual_backend::start_manual(runtime);
    pump.render(2);
    assert!(appears(&path));
    // The recorder keeps writing through its open file; building it again
    // would create the file anew.
    std::fs::remove_file(&path).unwrap();

    running
        .reload(doc(&with_recorder("220.0")))
        .expect("diff applies");
    pump.render(1);
    assert!(
        !path.exists(),
        "the reload rebuilt the recorder over its file"
    );
}

/// Waits, up to two seconds, for a recorder's writer thread to create
/// `path`: it opens the file once the recorder first processes audio.
fn appears(path: &std::path::Path) -> bool {
    for _ in 0..200 {
        if path.exists() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

/// Counts the live builds of an oscillator; validation builds are not live.
#[derive(Clone, Default)]
struct CountedOscillator(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl crate::ModuleFactory for CountedOscillator {
    fn type_id(&self) -> &'static str {
        "counted_oscillator"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<crate::factory::ModuleBuildResult, Box<dyn std::error::Error>> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.build_for_validation(sample_rate, config)
    }

    fn build_for_validation(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<crate::factory::ModuleBuildResult, Box<dyn std::error::Error>> {
        ModuleRegistry::default().build("oscillator", sample_rate, config)
    }
}

/// A reload checks the new document without building any module live: only
/// what the diff adds or rebuilds is built for real.
#[test]
fn reload_checks_the_document_without_live_builds() {
    let counted = CountedOscillator::default();
    let mut registry = ModuleRegistry::default();
    registry.register(counted.clone());
    let with_counted = |frequency: &str| {
        BASE.replace(
            r#"{ "id": "dac", "type": "dac" }"#,
            r#"{ "id": "dac", "type": "dac" }, { "id": "c", "type": "counted_oscillator" }"#,
        )
        .replace("440.0", frequency)
    };
    let (runtime, _) = InventionBuilder::with_registry(48_000, registry)
        .build(doc(&with_counted("440.0")))
        .unwrap();
    let mut running = runtime
        .start_with_backend(NullBackend::new(48_000))
        .unwrap();
    let live = || counted.0.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(live(), 1);

    running
        .reload(doc(&with_counted("220.0")))
        .expect("diff applies");
    assert_eq!(live(), 1, "the reload built an unchanged module live");
}
