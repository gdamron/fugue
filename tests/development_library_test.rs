mod support;

use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use fugue::{Invention, InventionBuilder};
use support::NullAudioBackend;

const SAMPLE_RATE: u32 = 48_000;

fn development_path(file_name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("developments")
        .join(file_name)
}

#[test]
fn voice_library_presets_load_as_standalone_developments() {
    for file_name in [
        "piano.json",
        "marimba.json",
        "vibraphone.json",
        "pluck.json",
        "pad.json",
    ] {
        let path = development_path(file_name);
        let invention = Invention::from_file(path.to_str().unwrap()).unwrap();

        assert!(
            invention.is_development(),
            "{file_name} should be a development"
        );
        assert_eq!(
            invention.inputs.len(),
            2,
            "{file_name} should expose two inputs"
        );
        assert_eq!(invention.inputs[0].name, "frequency");
        assert_eq!(invention.inputs[1].name, "gate");
        assert_eq!(
            invention.outputs.len(),
            1,
            "{file_name} should expose one output"
        );
        assert_eq!(invention.outputs[0].name, "audio");

        InventionBuilder::new(SAMPLE_RATE).build(invention).unwrap();
    }
}

#[test]
fn voice_library_trio_runs_multiple_development_instances() {
    let path = development_path("voice_library_trio.json");
    let invention = Invention::from_file(path.to_str().unwrap()).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(invention).unwrap();
    let running = runtime
        .start_with_backend(NullAudioBackend::new(SAMPLE_RATE))
        .unwrap();

    thread::sleep(Duration::from_millis(25));
    running.stop();
}
