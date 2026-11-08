//! Acceptance tests for a development's exposed control narrowing the range
//! of the inner control it aliases.
//!
//! An entry of a development's `controls` array may declare
//! `"range": { "min": .., "max": .. }`. The exposed control then reports that
//! range, clamps its starting value and every live write to it, and is
//! refused at load when the range is not within the aliased control's own.
//!
//! The development under test is a `kit`: an lfo `wobble` (inner `rate`,
//! 0.001-100 Hz) and a one-channel mixer `mix` (inner `level.0`, 0-2) fed a
//! constant, exposed as `rate`, `level` and `gain`.

use std::sync::{Arc, Mutex};

use fugue::rpc::StructuralEdit;
use fugue::{
    AudioBackend, ControlKind, ControlMeta, ControlValue, ControlWrite, DescribeModuleQuery,
    Invention, InventionBuilder, RenderEngine, RunningInvention,
};
use serde_json::{json, Value};

const RATE: u32 = 48_000;
const BLOCK: usize = 64;
/// The first 24 ms: a glide from a default to the set value would show.
const WINDOW: usize = 18 * BLOCK;

// -- documents -----------------------------------------------------------------

fn range(r: Option<(f64, f64)>) -> Value {
    r.map_or(Value::Null, |(min, max)| json!({ "min": min, "max": max }))
}

/// An exposed-control entry; `range` is left out when `None`.
fn exposed(name: &str, module: &str, control: &str, r: Option<(f64, f64)>) -> Value {
    let mut entry = json!({ "name": name, "module": module, "control": control });
    if let Some(r) = r {
        entry["range"] = range(Some(r));
    }
    entry
}

fn constant(id: &str) -> Value {
    json!({ "id": id, "type": "oscillator",
            "config": { "waveform": "square", "frequency": 0.001 } })
}

fn wire(from: &str, from_port: &str, to: &str, to_port: &str) -> Value {
    json!({ "from": from, "from_port": from_port, "to": to, "to_port": to_port })
}

/// The `kit` definition: `rate` and `level` take the given ranges, `gain`
/// aliases `level.0` too and never declares one.
fn kit(rate: Option<(f64, f64)>, level: Option<(f64, f64)>) -> Value {
    json!({
        "version": "1.0.0",
        "modules": [
            { "id": "wobble", "type": "lfo", "config": { "rate": 2.0, "waveform": "sine" } },
            constant("src"),
            { "id": "mix", "type": "mixer", "config": { "channel_count": 1 } },
        ],
        "connections": [ wire("src", "audio", "mix", "audio.0") ],
        "outputs": [ { "name": "audio", "from": "mix", "from_port": "audio_left" } ],
        "controls": [
            exposed("rate", "wobble", "rate", rate),
            exposed("level", "mix", "level.0", level),
            exposed("gain", "mix", "level.0", None),
        ],
    })
}

/// Two constants into a two-channel mixer, both channels behind one exposed
/// `level` (a fan-out); each entry declares the range given.
fn pair(first: Option<(f64, f64)>, second: Option<(f64, f64)>) -> Value {
    json!({
        "version": "1.0.0",
        "modules": [
            constant("a"),
            constant("b"),
            { "id": "mix", "type": "mixer", "config": { "channel_count": 2 } },
        ],
        "connections": [
            wire("a", "audio", "mix", "audio.0"),
            wire("b", "audio", "mix", "audio.1"),
        ],
        "outputs": [ { "name": "audio", "from": "mix", "from_port": "audio_left" } ],
        "controls": [
            exposed("level", "mix", "level.0", first),
            exposed("level", "mix", "level.1", second),
        ],
    })
}

/// A main graph holding one instance `v` of the development `kit`.
fn document(kit_definition: Value, config: Value) -> Value {
    json!({
        "version": "1.0.0",
        "developments": [ { "name": "kit", "definition": kit_definition } ],
        "modules": [
            { "id": "v", "type": "kit", "config": config },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } },
        ],
        "connections": [ wire("v", "audio", "dac", "audio") ],
    })
}

fn invention(doc: &Value) -> Invention {
    serde_json::from_value(doc.clone()).unwrap_or_else(|e| panic!("parses: {e}\n{doc:#}"))
}

// -- rigs ------------------------------------------------------------------------

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

/// A live invention driven by hand, block by block.
struct Live {
    running: RunningInvention,
    slot: Arc<Mutex<Option<Render>>>,
}

impl Live {
    fn start(doc: &Value) -> Live {
        let (runtime, _) = InventionBuilder::new(RATE)
            .build(invention(doc))
            .unwrap_or_else(|e| panic!("builds live: {e}\n{doc:#}"));
        let slot = Arc::new(Mutex::new(None));
        let running = runtime
            .start_with_backend(Capture(slot.clone()))
            .expect("a capturing backend starts");
        Live { running, slot }
    }

    fn render(&self, frames: usize) -> Vec<f32> {
        let mut slot = self.slot.lock().unwrap();
        let render = slot.as_mut().expect("backend started");
        let (mut left, mut right) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
        let mut out = Vec::new();
        for _ in 0..frames / BLOCK {
            render(&mut left, &mut right);
            out.extend_from_slice(&left);
        }
        out
    }
}

/// Either way of running a document, behind one set of operations.
enum Rig {
    Offline(RenderEngine),
    Live(Live),
}

impl Rig {
    fn both(doc: &Value) -> Vec<(&'static str, Rig)> {
        let mut engine = RenderEngine::new(RATE);
        engine
            .load_json(&doc.to_string())
            .unwrap_or_else(|e| panic!("loads offline: {e}\n{doc:#}"));
        engine.set_block_size(BLOCK);
        vec![
            ("offline", Rig::Offline(engine)),
            ("live", Rig::Live(Live::start(doc))),
        ]
    }

    fn render(&mut self, frames: usize) -> Vec<f32> {
        match self {
            Rig::Offline(engine) => {
                let mut buffer = vec![0.0f32; frames * 2];
                engine.render_interleaved(&mut buffer).expect("renders");
                buffer.as_chunks::<2>().0.iter().map(|f| f[0]).collect()
            }
            Rig::Live(live) => live.render(frames),
        }
    }

    /// A performed write to the instance's exposed control `key`.
    fn write(&mut self, key: &str, value: f32) {
        match self {
            Rig::Offline(e) => e
                .set_control("v", key, ControlValue::Number(value))
                .unwrap(),
            Rig::Live(l) => (l.running.set_control("v", key, ControlValue::Number(value))).unwrap(),
        }
    }

    /// The control `key` as read after the audio thread has had its blocks.
    fn read(&mut self, key: &str) -> f32 {
        self.render(4 * BLOCK);
        let value = match self {
            Rig::Offline(e) => e.get_control("v", key).unwrap(),
            Rig::Live(l) => l.running.get_control("v", key).unwrap(),
        };
        value.as_number().unwrap()
    }

    /// The live invention behind a rig (the offline engine has none).
    fn running(&mut self) -> &mut RunningInvention {
        match self {
            Rig::Live(l) => &mut l.running,
            Rig::Offline(_) => panic!("live only"),
        }
    }

    fn live() -> Rig {
        Rig::both(&document(ranged(), json!({}))).pop().unwrap().1
    }

    fn controls(&self) -> Vec<ControlMeta> {
        match self {
            Rig::Offline(e) => e.list_controls(Some("v")).unwrap().remove(0).1,
            Rig::Live(l) => l.running.list_controls("v").unwrap(),
        }
    }
}

// -- readings ----------------------------------------------------------------------

fn bounds(controls: &[ControlMeta], key: &str) -> (f32, f32) {
    let meta = controls
        .iter()
        .find(|c| c.key == key)
        .unwrap_or_else(|| panic!("no control {key}"));
    match meta.kind {
        ControlKind::Number { min, max, .. } => (min, max),
        _ => panic!("{key} is not a number control"),
    }
}

fn near(got: f32, want: f32) -> bool {
    (got - want).abs() <= 1e-4 * want.abs().max(1.0)
}

fn assert_bounds(controls: &[ControlMeta], key: &str, want: (f32, f32), label: &str) {
    let got = bounds(controls, key);
    assert!(
        near(got.0, want.0) && near(got.1, want.1),
        "{label}: {key} reports {got:?}, wanted {want:?}"
    );
}

fn assert_reads(rig: &mut Rig, key: &str, want: f32, label: &str) {
    let got = rig.read(key);
    assert!(near(got, want), "{label}: {key} reads {got}, wanted {want}");
}

fn max_abs(y: &[f32]) -> f32 {
    y.iter().fold(0.0, |m, v| m.max(v.abs()))
}

fn max_difference(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

/// The first `WINDOW` left-channel samples of `doc`, offline and live.
fn first_blocks(doc: &Value) -> [(&'static str, Vec<f32>); 2] {
    let mut rigs = Rig::both(doc);
    let live = rigs.pop().unwrap();
    let offline = rigs.pop().unwrap();
    [offline, live].map(|(name, mut rig)| (name, rig.render(WINDOW)))
}

const NARROWED: (f64, f64) = (0.05, 10.0);
const LEVEL: (f64, f64) = (0.0, 1.0);

fn ranged() -> Value {
    kit(Some(NARROWED), Some(LEVEL))
}

// -- 1. reported range -----------------------------------------------------------

#[test]
fn a_loaded_instance_reports_the_declared_range_and_the_aliased_one_otherwise() {
    for (mode, rig) in Rig::both(&document(ranged(), json!({}))) {
        let controls = rig.controls();
        assert_bounds(&controls, "rate", (0.05, 10.0), mode);
        assert_bounds(&controls, "level", (0.0, 1.0), mode);
        // No range declared: the aliased control's own, as today.
        assert_bounds(&controls, "gain", (0.0, 2.0), mode);
    }
}

#[test]
fn describe_module_reports_the_declared_range_for_an_instance_and_for_a_type_by_config() {
    let mut rig = Rig::live();
    let instance = rig
        .running()
        .describe_module(&DescribeModuleQuery {
            module_id: Some("v".into()),
            ..Default::default()
        })
        .expect("describes the instance");
    let by_config = rig
        .running()
        .describe_module(&DescribeModuleQuery {
            module_type: Some("kit".into()),
            config: Some(json!({ "rate": 3.0 })),
            ..Default::default()
        })
        .expect("describes the type with a config");
    for (label, description) in [("instance", instance), ("type by config", by_config)] {
        let controls = &description.info.controls;
        assert_bounds(controls, "rate", (0.05, 10.0), label);
        assert_bounds(controls, "level", (0.0, 1.0), label);
        assert_bounds(controls, "gain", (0.0, 2.0), label);
    }
}

#[test]
fn a_range_declared_on_one_entry_of_a_fan_out_reports_for_the_name() {
    for (first, second) in [
        (Some(LEVEL), None),
        (None, Some(LEVEL)),
        (Some(LEVEL), Some(LEVEL)),
    ] {
        let doc = document(pair(first, second), json!({}));
        for (mode, rig) in Rig::both(&doc) {
            let label = format!("{mode} {first:?}/{second:?}");
            assert_bounds(&rig.controls(), "level", (0.0, 1.0), &label);
        }
    }
}

// -- 2. starting values ----------------------------------------------------------

#[test]
fn a_configured_value_outside_the_range_is_clamped_when_built() {
    let cases = [
        (json!({ "rate": 100.0 }), "rate", 10.0),
        (json!({ "rate": 0.001 }), "rate", 0.05),
        (json!({ "rate": 3.0 }), "rate", 3.0),
        (json!({ "level": 1.5 }), "level", 1.0),
        (json!({ "level": 0.4 }), "level", 0.4),
        // No range: the value stands, as today.
        (json!({ "gain": 1.5 }), "gain", 1.5),
    ];
    for (config, key, want) in cases {
        for (mode, mut rig) in Rig::both(&document(ranged(), config.clone())) {
            assert_reads(&mut rig, key, want, &format!("{mode} config {config}"));
        }
    }
}

#[test]
fn the_first_audio_block_plays_the_clamped_starting_value() {
    let clamped = first_blocks(&document(ranged(), json!({ "level": 1.0 })));
    let over = first_blocks(&document(ranged(), json!({ "level": 1.5 })));
    let unranged = first_blocks(&document(kit(None, None), json!({ "level": 1.5 })));
    for i in 0..2 {
        let mode = clamped[i].0;
        assert!(
            max_abs(&clamped[i].1) > 0.01,
            "{mode}: the render is audible"
        );
        let off = max_difference(&over[i].1, &clamped[i].1);
        assert!(
            off <= 1e-5,
            "{mode}: 1.5 plays {off} away from the clamped 1.0"
        );
        let unclamped = max_difference(&unranged[i].1, &clamped[i].1);
        assert!(
            unclamped > 0.05,
            "{mode}: without a range 1.5 plays louder than 1.0"
        );
    }
}

#[test]
fn a_fan_out_clamps_every_target_to_the_range_declared_on_one_entry() {
    let reference = first_blocks(&document(pair(Some(LEVEL), None), json!({ "level": 1.0 })));
    for (first, second) in [(Some(LEVEL), None), (None, Some(LEVEL))] {
        let over = first_blocks(&document(pair(first, second), json!({ "level": 1.5 })));
        for i in 0..2 {
            let off = max_difference(&over[i].1, &reference[i].1);
            assert!(
                off <= 1e-5,
                "{} {first:?}/{second:?}: 1.5 plays {off} away from 1.0",
                over[i].0
            );
        }
    }
}

#[test]
fn describing_a_type_by_config_reports_the_clamped_starting_value() {
    let mut rig = Rig::live();
    let described = rig
        .running()
        .describe_module(&DescribeModuleQuery {
            module_type: Some("kit".into()),
            config: Some(json!({ "rate": 100.0, "level": 1.5 })),
            ..Default::default()
        })
        .expect("describes the type with a config");
    for (key, want) in [("rate", 10.0), ("level", 1.0)] {
        let got = described.control_values[key].as_number().unwrap();
        assert!(near(got, want), "{key} described as {got}, wanted {want}");
    }
}

// -- 3. live writes --------------------------------------------------------------

const WRITES: [(&str, f32, f32); 7] = [
    ("rate", 100.0, 10.0),
    ("rate", 0.001, 0.05),
    ("rate", 3.0, 3.0),
    ("level", 2.0, 1.0),
    ("level", 0.5, 0.5),
    // No range: only the aliased control's own range clamps.
    ("gain", 2.0, 2.0),
    ("rate", 1000.0, 10.0),
];

#[test]
fn set_control_outside_the_range_reads_back_clamped() {
    for (mode, mut rig) in Rig::both(&document(ranged(), json!({}))) {
        for (key, written, want) in WRITES {
            rig.write(key, written);
            assert_reads(&mut rig, key, want, &format!("{mode} wrote {written}"));
        }
    }
}

#[test]
fn set_controls_outside_the_range_reads_back_clamped() {
    let mut rig = Rig::live();
    // Authoring (the default over the wire) and performed writes alike.
    for performed in [false, true] {
        let write = |key: &str, value: f32| {
            let value = ControlValue::Number(value);
            if performed {
                ControlWrite::performed("v", key, value)
            } else {
                ControlWrite::new("v", key, value)
            }
        };
        let label = format!("performed={performed}");
        let acks = rig
            .running()
            .write_controls(&[write("rate", 100.0), write("level", 2.0)]);
        assert!(acks.error.is_none(), "writes accepted: {:?}", acks.error);
        assert_reads(&mut rig, "rate", 10.0, &label);
        assert_reads(&mut rig, "level", 1.0, &label);
        let acks = rig.running().write_controls(&[write("rate", 0.001)]);
        assert!(acks.error.is_none(), "writes accepted: {:?}", acks.error);
        assert_reads(&mut rig, "rate", 0.05, &label);
    }
}

#[test]
fn apply_edits_set_control_outside_the_range_reads_back_clamped() {
    let mut rig = Rig::live();
    for (key, written, want) in [
        ("rate", 100.0, 10.0),
        ("rate", 0.001, 0.05),
        ("level", 2.0, 1.0),
    ] {
        rig.running()
            .apply_edits(&[StructuralEdit::SetControl {
                module_id: "v".into(),
                key: key.into(),
                value: ControlValue::Number(written),
            }])
            .expect("the edit batch applies");
        assert_reads(&mut rig, key, want, &format!("apply_edits {written}"));
    }
}

#[test]
fn a_reload_with_a_changed_config_value_outside_the_range_reads_back_clamped() {
    let start = document(ranged(), json!({ "rate": 3.0, "level": 0.5 }));
    let mut rig = Rig::both(&start).pop().unwrap().1;
    let reloads = [
        (json!({ "rate": 100.0, "level": 0.5 }), "rate", 10.0),
        (json!({ "rate": 0.001, "level": 0.5 }), "rate", 0.05),
        (json!({ "rate": 0.001, "level": 2.0 }), "level", 1.0),
        (json!({ "rate": 7.0, "level": 0.25 }), "rate", 7.0),
    ];
    for (config, key, want) in reloads {
        rig.running()
            .reload(invention(&document(ranged(), config.clone())))
            .unwrap_or_else(|e| panic!("reloads {config}: {e}"));
        assert_reads(&mut rig, key, want, &format!("reload {config}"));
    }
}

// -- 4. refused ranges -----------------------------------------------------------

/// Every way a range can be wrong, with the numbers the error must name.
fn bad_ranges() -> Vec<(&'static str, Value, &'static str, Vec<&'static str>)> {
    vec![
        // Above the aliased lfo rate's 0.001-100.
        (
            "max above",
            kit(Some((0.5, 250.0)), None),
            "rate",
            vec!["0.5", "250", "0.001", "100"],
        ),
        // Below the aliased mixer level's 0-2.
        (
            "min below",
            kit(None, Some((-3.5, 1.5))),
            "level",
            vec!["-3.5", "1.5"],
        ),
        // Inverted.
        (
            "min above max",
            kit(Some((7.5, 2.5)), None),
            "rate",
            vec!["7.5", "2.5", "0.001", "100"],
        ),
    ]
}

#[test]
fn a_range_outside_the_aliased_range_or_inverted_is_refused_naming_everything() {
    for (label, definition, control, numbers) in bad_ranges() {
        let doc = document(definition, json!({}));
        let error = match InventionBuilder::new(RATE).build(invention(&doc)) {
            Ok(_) => panic!("{label}: a bad range was accepted"),
            Err(error) => error.to_string(),
        };
        for needle in ["kit", control].into_iter().chain(numbers) {
            assert!(
                error.contains(needle),
                "{label}: error lacks {needle:?}: {error}"
            );
        }
        // The offline engine refuses it the same way.
        let mut engine = RenderEngine::new(RATE);
        assert!(
            engine.load_json(&doc.to_string()).is_err(),
            "{label}: offline load"
        );
    }
}

#[test]
fn a_reload_to_a_bad_range_is_refused_and_leaves_the_instance_untouched() {
    let mut rig = Rig::both(&document(ranged(), json!({ "rate": 3.0 })))
        .pop()
        .unwrap()
        .1;
    for (label, definition, _, _) in bad_ranges() {
        let refused = rig
            .running()
            .reload(invention(&document(definition, json!({}))));
        assert!(refused.is_err(), "{label}: the reload was accepted");
        assert_reads(&mut rig, "rate", 3.0, label);
        assert_bounds(&rig.controls(), "rate", (0.05, 10.0), label);
    }
}

#[test]
fn a_range_equal_to_the_aliased_range_is_accepted() {
    let doc = document(kit(Some((0.001, 100.0)), Some((0.0, 2.0))), json!({}));
    for (mode, rig) in Rig::both(&doc) {
        assert_bounds(&rig.controls(), "rate", (0.001, 100.0), mode);
        assert_bounds(&rig.controls(), "level", (0.0, 2.0), mode);
    }
}

#[test]
fn disagreeing_ranges_on_one_name_are_refused() {
    let doc = document(pair(Some((0.0, 1.0)), Some((0.0, 1.5))), json!({}));
    assert!(InventionBuilder::new(RATE).build(invention(&doc)).is_err());
}

// -- 5. developments without ranges ----------------------------------------------

#[test]
fn a_development_without_ranges_describes_and_clamps_as_before() {
    let doc = document(kit(None, None), json!({}));
    for (mode, mut rig) in Rig::both(&doc) {
        let controls = rig.controls();
        assert_bounds(&controls, "rate", (0.001, 100.0), mode);
        assert_bounds(&controls, "level", (0.0, 2.0), mode);
        rig.write("rate", 1000.0);
        assert_reads(&mut rig, "rate", 100.0, mode);
        rig.write("level", 2.0);
        assert_reads(&mut rig, "level", 2.0, mode);
    }
}

#[test]
fn a_development_without_ranges_renders_as_the_same_wiring_flat() {
    let flat = json!({
        "version": "1.0.0",
        "modules": [
            constant("src"),
            { "id": "mix", "type": "mixer",
              "config": { "channel_count": 1, "levels": [1.5] } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } },
        ],
        "connections": [
            wire("src", "audio", "mix", "audio.0"),
            wire("mix", "audio_left", "dac", "audio"),
        ],
    });
    let want = first_blocks(&flat);
    let got = first_blocks(&document(kit(None, None), json!({ "level": 1.5 })));
    for i in 0..2 {
        assert!(max_abs(&want[i].1) > 0.01, "{}: audible", want[i].0);
        let off = max_difference(&got[i].1, &want[i].1);
        assert!(off <= 1e-6, "{}: {off} from the flat render", got[i].0);
    }
}

#[test]
fn serializing_leaves_a_range_out_unless_one_is_declared() {
    let plain = serde_json::to_value(invention(&document(kit(None, None), json!({})))).unwrap();
    for control in plain["developments"][0]["definition"]["controls"]
        .as_array()
        .unwrap()
    {
        assert!(
            control.get("range").is_none(),
            "a control without a range gained one: {control}"
        );
    }

    let saved = serde_json::to_value(invention(&document(ranged(), json!({})))).unwrap();
    let controls = saved["developments"][0]["definition"]["controls"]
        .as_array()
        .unwrap();
    let by_name = |name: &str| controls.iter().find(|c| c["name"] == name).unwrap();
    assert_eq!(by_name("rate")["range"]["min"].as_f64(), Some(0.05));
    assert_eq!(by_name("rate")["range"]["max"].as_f64(), Some(10.0));
    assert!(by_name("gain").get("range").is_none());
}
