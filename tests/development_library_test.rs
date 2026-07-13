mod support;

use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use fugue::{Invention, InventionBuilder, RenderEngine};
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
            3,
            "{file_name} should expose three inputs"
        );
        assert_eq!(invention.inputs[0].name, "frequency");
        assert_eq!(invention.inputs[1].name, "gate");
        assert_eq!(
            invention.inputs[2].name, "pedal",
            "{file_name} should route a pedal input to its sustain module"
        );
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

#[test]
#[ignore = "local performance check; run with --ignored --nocapture"]
fn voice_library_trio_tight_render_benchmark() {
    let path = development_path("voice_library_trio.json");
    let invention = Invention::from_file(path.to_str().unwrap()).unwrap();
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_invention(invention).unwrap();

    let frames = SAMPLE_RATE as usize * 2;
    let mut output = vec![0.0; frames * 2];
    let start = Instant::now();
    let rendered = engine.render_interleaved(&mut output).unwrap();
    let elapsed = start.elapsed();

    std::hint::black_box(output);
    eprintln!(
        "rendered {} frames from voice_library_trio.json in {:?} ({:.2}x realtime)",
        rendered,
        elapsed,
        (rendered as f64 / SAMPLE_RATE as f64) / elapsed.as_secs_f64()
    );
}

/// Compares render throughput at several block sizes. Block size 1 is the
/// legacy per-sample cadence; larger blocks should be at least as fast.
#[test]
#[ignore = "local performance check; run with --ignored --nocapture"]
fn render_throughput_across_block_sizes() {
    let path = development_path("voice_library_trio.json");
    let frames = SAMPLE_RATE as usize * 2;

    for &block in &[1usize, 16, 64, 256, 1024] {
        let invention = Invention::from_file(path.to_str().unwrap()).unwrap();
        let mut engine = RenderEngine::new(SAMPLE_RATE);
        engine.load_invention(invention).unwrap();
        engine.set_block_size(block);

        let mut output = vec![0.0; frames * 2];
        // Warm up, then measure.
        engine.render_interleaved(&mut output).unwrap();
        let start = Instant::now();
        let rendered = engine.render_interleaved(&mut output).unwrap();
        let elapsed = start.elapsed();
        std::hint::black_box(&output);

        eprintln!(
            "block_size={:>4}: {} frames in {:?} ({:.2}x realtime)",
            block,
            rendered,
            elapsed,
            (rendered as f64 / SAMPLE_RATE as f64) / elapsed.as_secs_f64()
        );
    }
}

/// A composite (DevelopmentModule) voice must render identically regardless of
/// block size: its internal sub-graph is processed full-block for acyclic
/// voices, and that must match the per-sample result.
#[test]
fn development_voice_block_size_parity() {
    // Marimba voice (adsr/filter/oscillator/vca — deterministic, no RNG) driven
    // by a clock gate; frequency input left unconnected so the oscillator's
    // control default is used.
    let marimba = std::fs::read_to_string(development_path("marimba.json")).unwrap();
    let json = format!(
        r#"{{
            "version": "1.0.0",
            "developments": [ {{ "name": "marimba_voice", "definition": {marimba} }} ],
            "modules": [
                {{ "id": "clock", "type": "clock", "config": {{ "bpm": 140.0, "gate_duration": 0.5 }} }},
                {{ "id": "voice", "type": "marimba_voice", "config": {{}} }},
                {{ "id": "dac", "type": "dac", "config": {{ "soft_clip": false }} }}
            ],
            "connections": [
                {{ "from": "clock", "from_port": "gate", "to": "voice", "to_port": "gate" }},
                {{ "from": "voice", "from_port": "audio", "to": "dac", "to_port": "audio" }}
            ]
        }}"#
    );

    let render = |block: usize| -> Vec<f32> {
        let mut engine = RenderEngine::new(SAMPLE_RATE);
        engine.load_json(&json).unwrap();
        engine.set_block_size(block);
        let mut out = vec![0.0f32; 4096 * 2];
        engine.render_interleaved(&mut out).unwrap();
        out
    };

    let per_sample = render(1);
    for &block in &[2usize, 64, 512] {
        let blocked = render(block);
        for (i, (x, y)) in per_sample.iter().zip(blocked.iter()).enumerate() {
            assert_eq!(
                x, y,
                "voice sample {i} differs between block 1 and {block} — \
                 DevelopmentModule block path diverged"
            );
        }
    }
    assert!(per_sample.iter().any(|s| s.abs() > 0.0), "voice was silent");
}

/// Renders the full `in_c` example (13 nested DevelopmentModule voices + a
/// 20-channel mixer + reverb) and reports realtime headroom and per-block max
/// time at the default block size — the live-underrun reproduction.
#[test]
#[ignore = "local performance check; run with --ignored --nocapture"]
fn in_c_render_throughput() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("in_c.json");
    let block = 64usize;
    let device_frames = 512usize; // emulate a typical cpal callback buffer
    let total_frames = SAMPLE_RATE as usize * 2;

    let invention = Invention::from_file(path.to_str().unwrap()).unwrap();
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_invention(invention).unwrap();
    engine.set_block_size(block);

    // Warm up (first call recompiles / settles control threads).
    let mut buf = vec![0.0f32; device_frames * 2];
    engine.render_interleaved(&mut buf).unwrap();

    let mut worst = Duration::ZERO;
    let mut total = Duration::ZERO;
    let mut rendered = 0usize;
    let deadline = Duration::from_secs_f64(device_frames as f64 / SAMPLE_RATE as f64);
    while rendered < total_frames {
        let start = Instant::now();
        engine.render_interleaved(&mut buf).unwrap();
        let elapsed = start.elapsed();
        std::hint::black_box(&buf);
        worst = worst.max(elapsed);
        total += elapsed;
        rendered += device_frames;
    }

    eprintln!(
        "in_c block={} device_buf={} frames: {} frames total in {:?} ({:.2}x realtime); \
         per-callback worst {:?} vs deadline {:?} ({:.1}% of budget)",
        block,
        device_frames,
        rendered,
        total,
        (rendered as f64 / SAMPLE_RATE as f64) / total.as_secs_f64(),
        worst,
        deadline,
        worst.as_secs_f64() / deadline.as_secs_f64() * 100.0
    );
}

/// Public-data stress case for the nested-development polyphony that exposed
/// the debug-runtime regression. Seventy-two piano voices share one sequencer,
/// matching the voice count without embedding the private composition.
#[test]
#[ignore = "local performance check; run with --ignored --nocapture"]
fn high_polyphony_nested_development_throughput() {
    const VOICES: usize = 72;
    let piano: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(development_path("piano.json")).unwrap())
            .unwrap();

    let mut modules = vec![
        serde_json::json!({
            "id": "clock",
            "type": "clock",
            "config": { "bpm": 120.0 }
        }),
        serde_json::json!({
            "id": "seq",
            "type": "step_sequencer",
            "config": {
                "base_note": 60,
                "steps": 1,
                "gate_length": 0.8,
                "pattern": [{ "note": 0, "gate": 0.8 }]
            }
        }),
    ];
    let mut connections = vec![serde_json::json!({
        "from": "clock",
        "from_port": "gate_x4",
        "to": "seq",
        "to_port": "gate"
    })];

    for voice in 0..VOICES {
        let id = format!("voice_{voice}");
        modules.push(serde_json::json!({ "id": &id, "type": "piano_voice" }));
        connections.push(serde_json::json!({
            "from": "seq", "from_port": "frequency", "to": &id, "to_port": "frequency"
        }));
        connections.push(serde_json::json!({
            "from": "seq", "from_port": "gate", "to": &id, "to_port": "gate"
        }));
        connections.push(serde_json::json!({
            "from": &id, "from_port": "audio", "to": "dac", "to_port": "audio"
        }));
    }
    modules.push(serde_json::json!({
        "id": "dac",
        "type": "dac",
        "config": { "soft_clip": false }
    }));

    let invention = serde_json::json!({
        "version": "1.0.0",
        "developments": [{ "name": "piano_voice", "definition": piano }],
        "modules": modules,
        "connections": connections
    });
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_json(&invention.to_string()).unwrap();

    let device_frames = 512usize;
    let total_frames = SAMPLE_RATE as usize * 2;
    let mut buffer = vec![0.0f32; device_frames * 2];
    engine.render_interleaved(&mut buffer).unwrap();

    let deadline = Duration::from_secs_f64(device_frames as f64 / SAMPLE_RATE as f64);
    let mut worst = Duration::ZERO;
    let start = Instant::now();
    let mut rendered = 0usize;
    while rendered < total_frames {
        let callback_start = Instant::now();
        engine.render_interleaved(&mut buffer).unwrap();
        worst = worst.max(callback_start.elapsed());
        rendered += device_frames;
    }
    let elapsed = start.elapsed();
    std::hint::black_box(buffer);

    eprintln!(
        "{VOICES}-voice nested piano: {rendered} frames in {elapsed:?} ({:.2}x realtime); \
         worst callback {worst:?} vs {deadline:?} ({:.1}% of budget)",
        (rendered as f64 / SAMPLE_RATE as f64) / elapsed.as_secs_f64(),
        worst.as_secs_f64() / deadline.as_secs_f64() * 100.0
    );
}
