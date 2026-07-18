use super::*;
use std::path::Path;

fn write_test_wav(path: &Path, sample_rate: u32, frames: &[[f32; 2]]) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for frame in frames {
        writer
            .write_sample((frame[0].clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .unwrap();
        writer
            .write_sample((frame[1].clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .unwrap();
    }
    writer.finalize().unwrap();
}

fn explicit_config(path: &Path) -> serde_json::Value {
    serde_json::json!({
        "asset": { "path": path.to_str().unwrap() },
        "slices": [
            { "start_frames": 0, "end_frames": 2, "name": "first" },
            { "start_frames": 2, "end_frames": 4, "name": "second" }
        ]
    })
}

#[test]
fn plays_selected_slice_to_exclusive_end() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("break.wav");
    write_test_wav(
        &path,
        44_100,
        &[[0.1, -0.1], [0.2, -0.2], [0.6, -0.6], [0.8, -0.8]],
    );
    let mut built = SampleSlicerFactory
        .build(44_100, &explicit_config(&path))
        .unwrap();
    let slicer = built.module.module_mut();

    slicer.set_input("slice", 1.0).unwrap();
    slicer.set_input("trigger", 1.0).unwrap();
    slicer.process(1);
    assert!(slicer.get_output("audio_left").unwrap() > 0.55);
    assert!(slicer.get_output("audio_right").unwrap() < -0.55);
    assert_eq!(slicer.get_output("slice_start_gate").unwrap(), 1.0);
    assert_eq!(slicer.get_output("slice_end_gate").unwrap(), 0.0);

    slicer.set_input("trigger", 0.0).unwrap();
    slicer.process(1);
    assert!(slicer.get_output("audio_left").unwrap() > 0.75);
    assert_eq!(slicer.get_output("slice_start_gate").unwrap(), 0.0);
    assert_eq!(slicer.get_output("slice_end_gate").unwrap(), 1.0);

    slicer.process(1);
    assert_eq!(slicer.get_output("audio_left").unwrap(), 0.0);
    assert_eq!(slicer.get_output("slice_end_gate").unwrap(), 0.0);
}

#[test]
fn retrigger_latches_the_new_slice() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("retrigger.wav");
    write_test_wav(
        &path,
        44_100,
        &[[0.1, 0.0], [0.2, 0.0], [0.7, 0.0], [0.8, 0.0]],
    );
    let mut built = SampleSlicerFactory
        .build(44_100, &explicit_config(&path))
        .unwrap();
    let slicer = built.module.module_mut();

    slicer.set_input("trigger", 1.0).unwrap();
    slicer.process(1);
    assert!(slicer.get_output("audio_left").unwrap() < 0.15);

    slicer.set_input("trigger", 0.0).unwrap();
    slicer.process(1);
    slicer.set_input("slice", 1.0).unwrap();
    slicer.set_input("trigger", 1.0).unwrap();
    slicer.process(1);
    assert!(slicer.get_output("audio_left").unwrap() > 0.65);
    assert_eq!(slicer.get_output("slice_start_gate").unwrap(), 1.0);
}

#[test]
fn scales_source_frame_slices_to_the_engine_rate() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("resampled.wav");
    write_test_wav(
        &path,
        22_050,
        &[[0.1, 0.0], [0.5, 0.0], [0.9, 0.0], [0.4, 0.0]],
    );
    let config = serde_json::json!({
        "source": path.to_str().unwrap(),
        "slices": [{ "start_frames": 1, "end_frames": 2 }]
    });
    let mut built = SampleSlicerFactory.build(44_100, &config).unwrap();
    let slicer = built.module.module_mut();

    slicer.set_input("trigger", 1.0).unwrap();
    slicer.process(1);
    assert_eq!(slicer.get_output("slice_end_gate").unwrap(), 0.0);
    slicer.set_input("trigger", 0.0).unwrap();
    slicer.process(1);
    assert_eq!(
        slicer.get_output("slice_end_gate").unwrap(),
        1.0,
        "one source frame becomes two frames at double the sample rate"
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn loads_slice_points_from_sample_pack_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let pack = temp.path().join("fugue.test.breaks").join("1.0.0");
    let samples_dir = pack.join("samples");
    std::fs::create_dir_all(&samples_dir).unwrap();
    let path = samples_dir.join("break.wav");
    write_test_wav(
        &path,
        44_100,
        &[[0.1, 0.0], [0.2, 0.0], [0.7, 0.0], [0.8, 0.0]],
    );
    std::fs::write(
        pack.join("fugue.pkg.json"),
        serde_json::to_vec(&serde_json::json!({
            "id": "fugue.test.breaks",
            "version": "1.0.0",
            "kind": "sample-pack",
            "license": "CC0-1.0",
            "authors": [{ "name": "Test" }],
            "targets": ["external-agent", "in-graph-agent"],
            "requires": { "capabilities": ["fs:read:samples/"] },
            "entry": { "samples": "samples.json" }
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        pack.join("samples.json"),
        serde_json::to_vec(&serde_json::json!({
            "license": "CC0-1.0",
            "sample_rate": [44100],
            "files": [{
                "path": "samples/break.wav",
                "slices": [
                    { "start_frames": 0, "end_frames": 2 },
                    { "start_frames": 2, "end_frames": 4, "name": "snare" }
                ]
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let config = serde_json::json!({ "asset": path.to_str().unwrap(), "slice": 1 });
    let mut built = SampleSlicerFactory.build(44_100, &config).unwrap();
    let slicer = built.module.module_mut();
    slicer.set_input("trigger", 1.0).unwrap();
    slicer.process(1);

    assert!(slicer.get_output("audio_left").unwrap() > 0.65);
    assert_eq!(slicer.get_output("slice_start_gate").unwrap(), 1.0);
}

#[test]
fn rejects_invalid_or_out_of_bounds_slices() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("invalid.wav");
    write_test_wav(&path, 44_100, &[[0.1, 0.0], [0.2, 0.0]]);

    let invalid_range = serde_json::json!({
        "source": path.to_str().unwrap(),
        "slices": [{ "start_frames": 1, "end_frames": 1 }]
    });
    let error = SampleSlicerFactory
        .build(44_100, &invalid_range)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("end_frames must exceed"), "{error}");

    let out_of_bounds = serde_json::json!({
        "source": path.to_str().unwrap(),
        "slices": [{ "start_frames": 0, "end_frames": 3 }]
    });
    let error = SampleSlicerFactory
        .build(44_100, &out_of_bounds)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("beyond sample length"), "{error}");
}

#[test]
fn registry_exposes_sample_slicer_ports_without_loading_an_asset() {
    let registry = crate::ModuleRegistry::default();
    assert!(registry.has_type("sample_slicer"));
    assert_eq!(
        registry.factory_input_ports("sample_slicer"),
        Some(inputs::INPUTS.as_slice())
    );
    assert_eq!(
        registry.factory_output_ports("sample_slicer"),
        Some(outputs::OUTPUTS.as_slice())
    );
}

fn elastic_config(path: &Path) -> serde_json::Value {
    let mut config = explicit_config(path);
    config["mode"] = serde_json::json!("elastic");
    config
}

/// Longer fixture so elastic slices have room to scale: 8 frames, two
/// 4-frame slices.
fn elastic_fixture(dir: &Path) -> (std::path::PathBuf, serde_json::Value) {
    let path = dir.join("elastic.wav");
    let frames: Vec<[f32; 2]> = (0..8).map(|i| [0.1 + i as f32 * 0.1, 0.0]).collect();
    write_test_wav(&path, 44_100, &frames);
    let config = serde_json::json!({
        "asset": { "path": path.to_str().unwrap() },
        "mode": "elastic",
        "slices": [
            { "start_frames": 0, "end_frames": 4 },
            { "start_frames": 4, "end_frames": 8 }
        ]
    });
    (path, config)
}

#[test]
fn elastic_mode_exposes_ratio_controls() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("controls.wav");
    write_test_wav(
        &path,
        44_100,
        &[[0.1, -0.1], [0.2, -0.2], [0.6, -0.6], [0.8, -0.8]],
    );

    // Classic build keeps the surface absent.
    let classic = SampleSlicerFactory
        .build(44_100, &explicit_config(&path))
        .unwrap();
    assert!(classic.control_surface.is_none());

    let elastic = SampleSlicerFactory
        .build(44_100, &elastic_config(&path))
        .unwrap();
    let surface = elastic.control_surface.unwrap();
    let keys: Vec<String> = surface
        .controls()
        .iter()
        .map(|meta| meta.key.clone())
        .collect();
    assert!(keys.contains(&"time_ratio".to_string()), "{keys:?}");
    assert!(keys.contains(&"pitch_ratio".to_string()), "{keys:?}");
}

#[test]
fn elastic_slice_matches_classic_gate_timing_at_unity() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("unity.wav");
    write_test_wav(
        &path,
        44_100,
        &[[0.1, -0.1], [0.2, -0.2], [0.6, -0.6], [0.8, -0.8]],
    );
    let mut built = SampleSlicerFactory
        .build(44_100, &elastic_config(&path))
        .unwrap();
    let slicer = built.module.module_mut();

    slicer.set_input("slice", 1.0).unwrap();
    slicer.set_input("trigger", 1.0).unwrap();
    slicer.process(1);
    assert!(slicer.get_output("audio_left").unwrap() > 0.55);
    assert!(slicer.get_output("audio_right").unwrap() < -0.55);
    assert_eq!(slicer.get_output("slice_start_gate").unwrap(), 1.0);
    assert_eq!(slicer.get_output("slice_end_gate").unwrap(), 0.0);

    slicer.set_input("trigger", 0.0).unwrap();
    slicer.process(1);
    assert!(slicer.get_output("audio_left").unwrap() > 0.75);
    assert_eq!(slicer.get_output("slice_end_gate").unwrap(), 1.0);

    slicer.process(1);
    assert_eq!(slicer.get_output("audio_left").unwrap(), 0.0);
    assert_eq!(slicer.get_output("slice_end_gate").unwrap(), 0.0);
}

#[test]
fn elastic_time_ratio_scales_slice_length() {
    let temp = tempfile::tempdir().unwrap();
    let (_path, config) = elastic_fixture(temp.path());
    let mut built = SampleSlicerFactory.build(44_100, &config).unwrap();
    let surface = built.control_surface.clone().unwrap();
    let slicer = built.module.module_mut();

    let frames_to_end = |slicer: &mut dyn Module| {
        slicer.set_input("trigger", 1.0).unwrap();
        for count in 1..100 {
            slicer.process(1);
            slicer.set_input("trigger", 0.0).unwrap();
            if slicer.get_output("slice_end_gate").unwrap() > 0.5 {
                return count;
            }
        }
        panic!("slice never ended");
    };

    // 4 source frames at half speed occupy 8 output frames; at double
    // speed, 2. Pitch alone must not change the count.
    assert_eq!(frames_to_end(slicer), 4);
    surface
        .set_control("time_ratio", crate::ControlValue::Number(0.5))
        .unwrap();
    assert_eq!(frames_to_end(slicer), 8);
    surface
        .set_control("time_ratio", crate::ControlValue::Number(2.0))
        .unwrap();
    assert_eq!(frames_to_end(slicer), 2);
    surface
        .set_control("time_ratio", crate::ControlValue::Number(1.0))
        .unwrap();
    surface
        .set_control("pitch_ratio", crate::ControlValue::Number(2.0))
        .unwrap();
    assert_eq!(frames_to_end(slicer), 4);
}

#[test]
fn elastic_rejects_unknown_mode() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("badmode.wav");
    write_test_wav(&path, 44_100, &[[0.1, 0.0], [0.2, 0.0]]);
    let mut config = serde_json::json!({
        "source": path.to_str().unwrap(),
        "slices": [{ "start_frames": 0, "end_frames": 2 }]
    });
    config["mode"] = serde_json::json!("granular");
    let err = SampleSlicerFactory
        .build(44_100, &config)
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("'mode'"), "{err}");
}
