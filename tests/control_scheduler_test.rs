//! Integration tests for the control_scheduler module: frame-exact scheduled
//! control changes, deterministic renders, live schedule edits, `$asset`
//! splicing, and load-time validation (FUG-186; the frame-exactness
//! discipline mirrors the FUG-176 end-gate tests).

use fugue::modules::mixer::gain_ramp_frames;
use fugue::{ControlValue, Invention, RenderEngine};

const SAMPLE_RATE: u32 = 48_000;
/// Frames between beat edges after the first (see SCHEDULED_CUT_INVENTION).
const STEP_FRAMES: usize = 128;
/// The step-2 edge, where the configured cut lands.
const STEP_2_EDGE: usize = 255;
/// How far the cut's first frame may be from the glide's first step,
/// relative to full level: f32 rounding only.
const CUT_TOLERANCE: f32 = 1e-4;

/// Frames the mixer takes to glide to a new level.
fn glide_frames() -> usize {
    gain_ramp_frames(SAMPLE_RATE) as usize
}

/// Frames to render to see a cut at `edge` through its glide and the next
/// gate after it.
fn frames_past_cut(edge: usize) -> usize {
    edge + glide_frames() + 2 * STEP_FRAMES
}

/// Asserts a cut of the mixer's level lands on exactly `edge`: there, the
/// output is the first step of the glide down from the same gate frame one
/// step earlier (had it landed a frame late, it would be at full level), and
/// once the glide is over the output is silent.
fn assert_cut_lands_on(output: &[f32], edge: usize) {
    let full = output[edge - STEP_FRAMES];
    assert!(full > 0.1, "the gate sounds a step before the cut");
    let first_step = full * (1.0 - 1.0 / glide_frames() as f32);
    assert!(
        (output[edge] - first_step).abs() <= full * CUT_TOLERANCE,
        "the cut starts its glide on frame {edge}: {} against {first_step}",
        output[edge]
    );
    assert!(
        output[edge + glide_frames()..]
            .iter()
            .all(|&sample| sample == 0.0),
        "everything after the glide stays silent"
    );
}

/// Clock at 22_500 BPM = 128 samples per beat at 48 kHz. The clock
/// pre-increments its sample counter, so beat edges land at frames 0, 127,
/// 255, 383, 511 (the first period is one sample shorter): step `k` begins
/// at frame `128 * k - 1` for `k >= 1`.
///
/// The clock's own beat gate feeds mixer channel 1 (high for 32 frames from
/// each beat edge), so with the scheduled cut of `level.0` at step 2 the
/// output is nonzero during the step-1 gate, starts the mixer's level glide
/// on exactly the step-2 edge (frame 255), and is silent once it is over.
const SCHEDULED_CUT_INVENTION: &str = r#"{
    "version": "1.0.0",
    "title": "scheduled-cut-test",
    "modules": [
        { "id": "clock", "type": "clock", "config": { "bpm": 22500.0 } },
        { "id": "mixer", "type": "mixer", "config": { "channel_count": 1 } },
        {
            "id": "sched",
            "type": "control_scheduler",
            "config": {
                "schedule": [
                    { "at_step": 2, "module": "mixer", "control": "level.0", "value": 0.0 }
                ]
            }
        },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "clock", "from_port": "beat", "to": "sched", "to_port": "clock" },
        { "from": "clock", "from_port": "beat", "to": "mixer", "to_port": "audio.0" },
        { "from": "mixer", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" },
        { "from": "mixer", "from_port": "audio_right", "to": "dac", "to_port": "audio_right" }
    ]
}"#;

/// Renders `frames` mono-summed output frames one block at a time.
fn render_frames(engine: &mut RenderEngine, frames: usize) -> Vec<f32> {
    let block = engine.block_size();
    let mut buffer = vec![0.0f32; block * 2];
    let mut output = Vec::with_capacity(frames);
    while output.len() < frames {
        let n = (frames - output.len()).min(block);
        engine.render_interleaved(&mut buffer[..n * 2]).unwrap();
        for k in 0..n {
            output.push(buffer[k * 2] + buffer[k * 2 + 1]);
        }
    }
    output
}

#[test]
fn scheduled_change_lands_on_the_exact_expected_frame() {
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_json(SCHEDULED_CUT_INVENTION).unwrap();
    engine.set_block_size(1);

    let output = render_frames(&mut engine, frames_past_cut(STEP_2_EDGE));

    // Sanity: the step-1 gate (frames 127..158) sounds at full level.
    assert!(
        output[127] > 0.1 && output[140] > 0.1,
        "step-1 gate must be audible before the scheduled cut"
    );
    assert_cut_lands_on(&output, STEP_2_EDGE);
    assert!(
        output[254] == 0.0 && output[158] > 0.0,
        "silence before the edge comes only from the gate being low"
    );
}

#[test]
fn scheduled_renders_are_byte_identical() {
    let render = || {
        let mut engine = RenderEngine::new(48_000);
        engine
            .load_json(
                r#"{
                "version": "1.0.0",
                "modules": [
                    { "id": "clock", "type": "clock", "config": { "bpm": 22500.0 } },
                    { "id": "mixer", "type": "mixer", "config": { "channel_count": 1 } },
                    {
                        "id": "sched",
                        "type": "control_scheduler",
                        "config": {
                            "schedule": [
                                { "at_step": 1, "module": "mixer", "control": "level.0", "value": 0.1, "ramp_steps": 4 },
                                { "at_step": 6, "module": "mixer", "control": "level.0", "value": 1.0 },
                                { "at_step": 8, "module": "clock", "control": "bpm", "value": 11250.0 }
                            ]
                        }
                    },
                    { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
                ],
                "connections": [
                    { "from": "clock", "from_port": "beat", "to": "sched", "to_port": "clock" },
                    { "from": "clock", "from_port": "beat", "to": "mixer", "to_port": "audio.0" },
                    { "from": "mixer", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" },
                    { "from": "mixer", "from_port": "audio_right", "to": "dac", "to_port": "audio_right" }
                ]
            }"#,
            )
            .unwrap();
        render_frames(&mut engine, 4_096)
    };

    let first: Vec<u32> = render().into_iter().map(f32::to_bits).collect();
    let second: Vec<u32> = render().into_iter().map(f32::to_bits).collect();
    assert_eq!(first, second, "two renders must be byte-identical");
}

#[test]
fn scheduling_the_driving_clock_changes_tempo() {
    // The scheduler both listens to the clock's gate and writes its bpm — a
    // cycle, so the graph processes both sample-by-sample as a feedback
    // group. The bpm must change exactly once step 2 is reached.
    let mut engine = RenderEngine::new(48_000);
    engine
        .load_json(
            r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "clock", "type": "clock", "config": { "bpm": 22500.0 } },
                {
                    "id": "sched",
                    "type": "control_scheduler",
                    "config": {
                        "schedule": [
                            { "at_step": 2, "module": "clock", "control": "bpm", "value": 45000.0 }
                        ]
                    }
                },
                { "id": "dac", "type": "dac" }
            ],
            "connections": [
                { "from": "clock", "from_port": "beat", "to": "sched", "to_port": "clock" }
            ]
        }"#,
        )
        .unwrap();
    engine.set_block_size(1);

    render_frames(&mut engine, 255);
    assert_eq!(
        engine.get_control("clock", "bpm").unwrap(),
        ControlValue::Number(22500.0),
        "tempo unchanged before step 2"
    );

    render_frames(&mut engine, 1); // frame 255: the step-2 edge
    assert_eq!(
        engine.get_control("sched", "step").unwrap(),
        ControlValue::Number(2.0)
    );
    // The clock declares its controls, so the write waits in its automation
    // slot and applies as the clock processes its next sample.
    render_frames(&mut engine, 1);
    assert_eq!(
        engine.get_control("clock", "bpm").unwrap(),
        ControlValue::Number(45000.0),
        "tempo changes from the sample after the step-2 edge"
    );
}

#[test]
fn schedule_can_be_replaced_during_playback() {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(SCHEDULED_CUT_INVENTION).unwrap();
    engine.set_block_size(1);

    // Replace the configured schedule (cut at step 2) before it fires: the
    // new schedule restores full level at step 1 and cuts at step 4 instead.
    engine
        .set_control(
            "sched",
            "schedule",
            ControlValue::String(
                r#"[{ "at_step": 4, "module": "mixer", "control": "level.0", "value": 0.0 }]"#
                    .to_string(),
            ),
        )
        .unwrap();

    let step_4_edge = STEP_2_EDGE + 2 * STEP_FRAMES;
    let output = render_frames(&mut engine, frames_past_cut(step_4_edge));
    assert!(
        output[STEP_2_EDGE] > 0.1,
        "the replaced schedule no longer cuts at step 2"
    );
    assert_cut_lands_on(&output, step_4_edge);

    // A schedule that fails to resolve is rejected and leaves the old one.
    let err = engine
        .set_control(
            "sched",
            "schedule",
            ControlValue::String(
                r#"[{ "at_step": 9, "module": "ghost", "control": "level.0", "value": 1.0 }]"#
                    .to_string(),
            ),
        )
        .unwrap_err();
    assert!(err.to_string().contains("unknown module"), "{}", err);
}

#[test]
fn schedules_splice_in_via_assets() {
    let dir = std::env::temp_dir().join(format!(
        "fugue-control-scheduler-asset-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let schedule_path = dir.join("dynamics.json");
    std::fs::write(
        &schedule_path,
        r#"[{ "at_step": 2, "module": "mixer", "control": "level.0", "value": 0.0 }]"#,
    )
    .unwrap();
    let invention_path = dir.join("invention.json");
    std::fs::write(
        &invention_path,
        r#"{
            "version": "1.0.0",
            "assets": { "dynamics": { "path": "dynamics.json" } },
            "modules": [
                { "id": "clock", "type": "clock", "config": { "bpm": 22500.0 } },
                { "id": "mixer", "type": "mixer", "config": { "channel_count": 1 } },
                {
                    "id": "sched",
                    "type": "control_scheduler",
                    "config": { "schedule": { "$asset": "dynamics" } }
                },
                { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
            ],
            "connections": [
                { "from": "clock", "from_port": "beat", "to": "sched", "to_port": "clock" },
                { "from": "clock", "from_port": "beat", "to": "mixer", "to_port": "audio.0" },
                { "from": "mixer", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" },
                { "from": "mixer", "from_port": "audio_right", "to": "dac", "to_port": "audio_right" }
            ]
        }"#,
    )
    .unwrap();

    let invention = Invention::from_file(invention_path.to_str().unwrap()).unwrap();
    let mut engine = RenderEngine::new(48_000);
    engine.load_invention(invention).unwrap();
    engine.set_block_size(1);

    let output = render_frames(&mut engine, frames_past_cut(STEP_2_EDGE));
    assert!(output[140] > 0.1, "spliced schedule leaves step 1 audible");
    assert_cut_lands_on(&output, STEP_2_EDGE);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn default_config_builds_and_curated_example_plays() {
    // No config at all (the shape `describe_module_types` builds with).
    let mut engine = RenderEngine::new(48_000);
    engine
        .load_json(
            r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "sched", "type": "control_scheduler" },
                { "id": "dac", "type": "dac" }
            ],
            "connections": []
        }"#,
        )
        .unwrap();
    assert_eq!(
        engine.get_control("sched", "schedule").unwrap(),
        ControlValue::String("[]".to_string())
    );

    // The curated example loads, sounds, and lands its tempo change
    // (120 -> 90 BPM at step 24, i.e. 12 seconds in).
    let invention = Invention::from_file("examples/control_scheduler.json").unwrap();
    let mut engine = RenderEngine::new(48_000);
    engine.load_invention(invention).unwrap();
    let output = render_frames(&mut engine, 600_000);
    assert!(
        output.iter().any(|&sample| sample != 0.0),
        "example must make sound"
    );
    assert_eq!(
        engine.get_control("clock", "bpm").unwrap(),
        ControlValue::Number(90.0),
        "the scheduled tempo change landed"
    );
}

#[test]
fn unresolvable_schedules_fail_at_load() {
    let mut engine = RenderEngine::new(48_000);
    let err = engine
        .load_json(
            r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "clock", "type": "clock" },
                {
                    "id": "sched",
                    "type": "control_scheduler",
                    "config": {
                        "schedule": [
                            { "at_step": 0, "module": "ghost", "control": "level.0", "value": 1.0 }
                        ]
                    }
                },
                { "id": "dac", "type": "dac" }
            ],
            "connections": [
                { "from": "clock", "from_port": "beat", "to": "sched", "to_port": "clock" }
            ]
        }"#,
        )
        .unwrap_err();
    assert!(err.to_string().contains("unknown module"), "{}", err);
}

#[test]
fn the_scheduler_refuses_its_pre_convention_names() {
    let registry = fugue::ModuleRegistry::default();
    for config in [
        serde_json::json!({ "tempo_target": "clock" }),
        serde_json::json!({ "bpm_scale": 2.0 }),
    ] {
        assert!(
            registry
                .build("control_scheduler", 48_000, &config)
                .is_err(),
            "{config}"
        );
    }
    let config = serde_json::json!({ "tempo_module": "clock", "tempo_scale": 2.0 });
    assert!(registry.build("control_scheduler", 48_000, &config).is_ok());

    let mut engine = RenderEngine::new(48_000);
    let err = engine
        .load_json(
            r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "clock", "type": "clock" },
                { "id": "sched", "type": "control_scheduler" }
            ],
            "connections": [
                { "from": "clock", "from_port": "beat", "to": "sched", "to_port": "gate" }
            ]
        }"#,
        )
        .unwrap_err();
    assert!(err.to_string().contains("gate"), "{}", err);
}
