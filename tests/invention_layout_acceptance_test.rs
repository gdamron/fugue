//! Acceptance tests for an invention's saved canvas layout and the
//! `move_modules` structural edit.
//!
//! Written from the spec alone. Criteria (numbers are the spec's):
//!
//! 1. An invention without a layout loads, saves and plays as before; a save
//!    writes no `layout` key.
//! 2. An invention with a layout keeps its positions exactly through save and
//!    load.
//! 3. Moving modules with the edit op changes only the document: the retained
//!    document has the new positions, no module is rebuilt, and the audio
//!    continues identically to an unedited twin.
//! 4. Removing a module (by edit op, or by reloading a document without it)
//!    removes its stored position.
//! 5. Unknown module ids in a document's layout are dropped with a warning,
//!    not a load failure.
//! 6. A reload whose document differs only in layout rebuilds nothing, and the
//!    retained document takes the new layout.
//! 7. A move made while a timed structural edit is waiting neither cancels
//!    that edit nor is lost when it lands.
//!
//! Plus the edit op's refusals: unknown module, empty map, non-finite
//! coordinate, more than 1024 positions, a timed batch of only moves, and an
//! unknown field inside a position.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fugue::rpc::{
    ApplyEditsReport, EditFailure, EditFailureReason, EditOp, EditWhen, RequestState,
    StructuralEdit, MAX_MOVES_PER_EDIT,
};
use fugue::{
    AudioBackend, Invention, InventionBuilder, ReloadReport, RenderEngine, RpcError, RpcErrorCode,
    RunningInvention,
};
use serde_json::{json, Value};

const SAMPLE_RATE: u32 = 48_000;
const BLOCK: usize = 64;

// ---------------------------------------------------------------------------
// Rig: a backend whose blocks the test renders by hand and records, so two
// running twins can be compared sample for sample.
// ---------------------------------------------------------------------------

type RenderFn = Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>;

#[derive(Default)]
struct Shared {
    render: Option<RenderFn>,
    recorded: Vec<f32>,
}

#[derive(Clone, Default)]
struct Pump(Arc<Mutex<Shared>>);

struct PumpBackend(Pump);

impl AudioBackend for PumpBackend {
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    fn start(&mut self, render: RenderFn) -> Result<(), Box<dyn std::error::Error>> {
        self.0 .0.lock().unwrap().render = Some(render);
        Ok(())
    }

    fn stop(&mut self) {
        self.0 .0.lock().unwrap().render = None;
    }
}

struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl Pump {
    /// Renders `blocks` blocks, recording left and right interleaved.
    fn render(&self, blocks: usize) {
        let mut shared = self.0.lock().unwrap();
        for _ in 0..blocks {
            let mut left = [0.0f32; BLOCK];
            let mut right = [0.0f32; BLOCK];
            let shared = &mut *shared;
            let render = shared.render.as_mut().expect("backend started");
            render(&mut left, &mut right);
            for frame in 0..BLOCK {
                shared.recorded.push(left[frame]);
                shared.recorded.push(right[frame]);
            }
        }
    }

    fn recorded(&self) -> Vec<f32> {
        self.0.lock().unwrap().recorded.clone()
    }

    /// Runs `body` while a helper thread keeps rendering blocks, so a call
    /// that waits on the audio thread cannot deadlock. A few blocks only: the
    /// helper pauses between blocks.
    fn while_pumping<R>(&self, body: impl FnOnce() -> R) -> R {
        let stop = Arc::new(AtomicBool::new(false));
        std::thread::scope(|scope| {
            let _guard = StopOnDrop(stop.clone());
            let pump = self.clone();
            let flag = stop.clone();
            scope.spawn(move || {
                while !flag.load(Ordering::SeqCst) {
                    pump.render(1);
                    std::thread::sleep(Duration::from_micros(200));
                }
            });
            body()
        })
    }
}

fn start(json: &str) -> (RunningInvention, Pump) {
    let invention = Invention::from_json(json).expect("document parses");
    start_invention(invention)
}

fn start_invention(invention: Invention) -> (RunningInvention, Pump) {
    let pump = Pump::default();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE)
        .build(invention)
        .expect("document builds");
    let running = runtime
        .start_with_backend(PumpBackend(pump.clone()))
        .expect("starts");
    (running, pump)
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()))
}

/// The two recordings agree bit for bit over their common length, and that
/// length is at least `at_least` samples.
fn assert_same_audio(edited: &[f32], twin: &[f32], at_least: usize) {
    let common = edited.len().min(twin.len());
    assert!(
        common >= at_least,
        "recorded only {common} samples, wanted {at_least}"
    );
    if let Some(index) = (0..common).find(|&i| edited[i].to_bits() != twin[i].to_bits()) {
        panic!(
            "audio diverged from the twin at sample {index}: {} vs {}",
            edited[index], twin[index]
        );
    }
    assert!(
        peak(&twin[..common]) > 0.01,
        "twin is silent; the comparison would prove nothing"
    );
}

// ---------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------

/// `lfo` (stateful) modulates `osc`, which plays into `dac`. `aux` is a
/// module nothing is connected to, free to remove.
fn document(layout: Option<Value>) -> String {
    let mut document = json!({
        "version": "1.0.0",
        "title": "layout acceptance",
        "modules": [
            { "id": "lfo", "type": "lfo", "config": { "rate": 3.0, "waveform": "sine" } },
            { "id": "osc", "type": "oscillator", "config": { "waveform": "sine", "frequency": 220.0 } },
            { "id": "aux", "type": "lfo", "config": { "rate": 0.5, "waveform": "sine" } },
            { "id": "dac", "type": "dac", "config": {} }
        ],
        "connections": [
            { "from": "lfo", "from_port": "bipolar", "to": "osc", "to_port": "frequency" },
            { "from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]
    });
    if let Some(layout) = layout {
        document["layout"] = layout;
    }
    document.to_string()
}

/// Same graph as [`document`] without `aux`.
fn document_without_aux(layout: Option<Value>) -> String {
    let mut value: Value = serde_json::from_str(&document(layout)).unwrap();
    let modules = value["modules"].as_array_mut().unwrap();
    modules.retain(|module| module["id"] != "aux");
    value.to_string()
}

fn layout_of(pairs: &[(&str, f64, f64)]) -> Value {
    let modules: serde_json::Map<String, Value> = pairs
        .iter()
        .map(|(id, x, y)| (id.to_string(), json!({ "x": x, "y": y })))
        .collect();
    json!({ "modules": modules })
}

fn edit(value: Value) -> StructuralEdit {
    serde_json::from_value(value).expect("edit parses")
}

fn move_edit(positions: Value) -> StructuralEdit {
    edit(json!({ "op": "move_modules", "positions": positions }))
}

/// The retained document's position for `id`, as `(x, y)`.
fn position(running: &RunningInvention, id: &str) -> Option<(f64, f64)> {
    let document = running.document().expect("document retained");
    document.layout.modules.get(id).map(|p| (p.x, p.y))
}

fn layout_ids(running: &RunningInvention) -> Vec<String> {
    let document = running.document().expect("document retained");
    document.layout.modules.keys().cloned().collect()
}

fn failure(error: &RpcError) -> &EditFailure {
    assert_eq!(error.code, RpcErrorCode::InvalidEdit, "{error:?}");
    error
        .edit
        .as_deref()
        .expect("an invalid edit names its edit")
}

fn saved_json(document: &Invention, dir: &Path) -> Value {
    let path = dir.join("saved.json");
    document.save_to_file(&path).unwrap();
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

// ---------------------------------------------------------------------------
// Criterion 1: no layout, as before
// ---------------------------------------------------------------------------

#[test]
fn c1_invention_without_layout_plays_and_saves_without_a_layout_key() {
    let (running, pump) = start(&document(None));
    pump.render(100);
    assert!(peak(&pump.recorded()) > 0.01, "plays");

    let retained = running.document().expect("document retained");
    assert!(retained.layout.modules.is_empty());
    assert!(!retained.to_json().unwrap().contains("\"layout\""));

    let to_save = running.document_to_save().expect("document retained");
    let dir = tempfile::tempdir().unwrap();
    let saved = saved_json(&to_save, dir.path());
    assert!(
        saved.get("layout").is_none(),
        "save wrote a layout key: {saved}"
    );
    running.stop();
}

#[test]
fn c1_offline_engine_loads_a_layoutless_document_and_saves_without_a_layout_key() {
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_json(&document(None)).unwrap();
    let mut out = vec![0.0f32; 2 * 4096];
    engine.render_interleaved(&mut out).unwrap();
    assert!(peak(&out) > 0.01);
    assert!(engine.load_warnings().is_empty());
    let document = engine.document().unwrap();
    assert!(document.layout.modules.is_empty());
    let json: Value = serde_json::from_str(&document.to_json().unwrap()).unwrap();
    assert!(json.get("layout").is_none(), "{json}");
}

#[test]
fn c1_a_layout_never_changes_what_is_heard() {
    let layout = layout_of(&[("osc", 120.5, -40.0), ("lfo", 0.0, 0.0)]);
    let mut with = RenderEngine::new(SAMPLE_RATE);
    with.load_json(&document(Some(layout))).unwrap();
    let mut without = RenderEngine::new(SAMPLE_RATE);
    without.load_json(&document(None)).unwrap();

    let mut a = vec![0.0f32; 2 * 8192];
    let mut b = vec![0.0f32; 2 * 8192];
    with.render_interleaved(&mut a).unwrap();
    without.render_interleaved(&mut b).unwrap();
    assert_same_audio(&a, &b, a.len());
}

// ---------------------------------------------------------------------------
// Criterion 2: positions survive save and load exactly
// ---------------------------------------------------------------------------

/// Numbers chosen to catch lossy formatting: not exactly representable in
/// decimal, tiny, huge, negative, integral.
const AWKWARD: &[(&str, f64, f64)] = &[
    ("lfo", 120.5, -40.0),
    ("osc", 0.1 + 0.2, 1e-7),
    ("aux", 123_456_789.5, -1e15),
    ("dac", 2.5e-300, 0.0),
];

#[test]
fn c2_layout_loads_into_the_retained_document() {
    let (running, _pump) = start(&document(Some(layout_of(AWKWARD))));
    for &(id, x, y) in AWKWARD {
        assert_eq!(position(&running, id), Some((x, y)), "{id}");
    }
    running.stop();
}

#[test]
fn c2_layout_survives_save_and_load_exactly() {
    let (running, _pump) = start(&document(Some(layout_of(AWKWARD))));
    let to_save = running.document_to_save().expect("document retained");
    running.stop();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("piece.json");
    to_save.save_to_file(&path).unwrap();

    // The file holds the positions in the documented shape.
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for &(id, x, y) in AWKWARD {
        let saved = &file["layout"]["modules"][id];
        assert_eq!(saved["x"].as_f64(), Some(x), "{id}.x in file");
        assert_eq!(saved["y"].as_f64(), Some(y), "{id}.y in file");
    }

    // And loading the file gives them back bit for bit.
    let reloaded = Invention::from_file(path.to_str().unwrap()).unwrap();
    for &(id, x, y) in AWKWARD {
        let position = reloaded.layout.modules.get(id).expect(id);
        assert_eq!(position.x.to_bits(), x.to_bits(), "{id}.x");
        assert_eq!(position.y.to_bits(), y.to_bits(), "{id}.y");
    }
    assert_eq!(reloaded.layout.modules.len(), AWKWARD.len());

    // A second save-load cycle is a fixed point.
    let (again, _pump) = start_invention(reloaded.clone());
    let second = again.document_to_save().unwrap();
    again.stop();
    assert_eq!(second.layout, reloaded.layout);
}

#[test]
fn c2_moved_positions_survive_save_and_load() {
    let (mut running, _pump) = start(&document(Some(layout_of(&[("osc", 1.0, 2.0)]))));
    running
        .apply_edits(&[move_edit(json!({
            "osc": { "x": -0.75, "y": 1e3 },
            "lfo": { "x": 3.25, "y": 4.5 }
        }))])
        .unwrap();
    let to_save = running.document_to_save().unwrap();
    running.stop();

    let dir = tempfile::tempdir().unwrap();
    let saved = saved_json(&to_save, dir.path());
    assert_eq!(saved["layout"]["modules"]["osc"]["x"].as_f64(), Some(-0.75));
    assert_eq!(saved["layout"]["modules"]["osc"]["y"].as_f64(), Some(1e3));
    assert_eq!(saved["layout"]["modules"]["lfo"]["x"].as_f64(), Some(3.25));
    assert_eq!(saved["layout"]["modules"]["lfo"]["y"].as_f64(), Some(4.5));
}

/// A position written with the full 17 significant digits of an f64 comes
/// back as the same f64, bit for bit (the JSON reader must round-trip
/// floats exactly, not within an ulp).
#[test]
fn c2_full_precision_positions_survive_load_and_save_exactly() {
    let precise = [
        123_456_789.123_456_79_f64,
        0.1234567890123456_7,
        -9.87654321098765_4e5,
    ];
    let pairs: Vec<(&str, f64, f64)> = vec![
        ("osc", precise[0], precise[1]),
        ("lfo", precise[2], std::f64::consts::PI),
    ];
    let (running, _pump) = start(&document(Some(layout_of(&pairs))));
    let retained = running.document_to_save().unwrap();
    running.stop();
    for &(id, x, y) in &pairs {
        let position = retained.layout.modules.get(id).expect(id);
        assert_eq!(position.x.to_bits(), x.to_bits(), "{id}.x on load");
        assert_eq!(position.y.to_bits(), y.to_bits(), "{id}.y on load");
    }

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("precise.json");
    retained.save_to_file(&path).unwrap();
    let reloaded = Invention::from_file(path.to_str().unwrap()).unwrap();
    for &(id, x, y) in &pairs {
        let position = reloaded.layout.modules.get(id).expect(id);
        assert_eq!(position.x.to_bits(), x.to_bits(), "{id}.x after save");
        assert_eq!(position.y.to_bits(), y.to_bits(), "{id}.y after save");
    }
}

#[test]
fn c2_an_unknown_field_inside_a_position_is_refused() {
    let with_z = document(Some(
        json!({ "modules": { "osc": { "x": 1.0, "y": 2.0, "z": 3.0 } } }),
    ));
    assert!(Invention::from_json(&with_z).is_err(), "document refused");
    assert!(RenderEngine::new(SAMPLE_RATE).load_json(&with_z).is_err());

    let position = serde_json::from_value::<fugue::invention::Position>(
        json!({ "x": 1.0, "y": 2.0, "z": 3.0 }),
    );
    assert!(position.is_err(), "position refused");

    let wire = serde_json::from_value::<StructuralEdit>(json!({
        "op": "move_modules",
        "positions": { "osc": { "x": 1.0, "y": 2.0, "z": 3.0 } }
    }));
    assert!(wire.is_err(), "move_modules position refused");
}

// ---------------------------------------------------------------------------
// The wire shape of the op
// ---------------------------------------------------------------------------

#[test]
fn move_modules_parses_from_its_documented_wire_shape() {
    let parsed: StructuralEdit = serde_json::from_str(
        r#"{ "op": "move_modules", "positions": { "osc": { "x": 120, "y": 40 }, "vca": null } }"#,
    )
    .unwrap();
    assert_eq!(parsed.op(), EditOp::MoveModules);
    assert_eq!(parsed.op().as_str(), "move_modules");
    let StructuralEdit::MoveModules { positions } = parsed else {
        panic!("not a move_modules edit");
    };
    let osc = positions["osc"].expect("osc placed");
    assert_eq!((osc.x, osc.y), (120.0, 40.0));
    assert_eq!(positions["vca"], None);
    assert_eq!(MAX_MOVES_PER_EDIT, 1024);
}

// ---------------------------------------------------------------------------
// Criterion 3: moving changes only the document
// ---------------------------------------------------------------------------

#[test]
fn c3_move_updates_the_retained_document_and_nothing_else() {
    let (mut running, pump) = start(&document(Some(layout_of(&[("osc", 1.0, 2.0)]))));
    pump.render(50);
    let before = running.full_snapshot();

    let report: ApplyEditsReport = running
        .while_pumping_edit(
            &pump,
            &[move_edit(json!({
                "osc": { "x": 120.5, "y": -40 },
                "aux": { "x": 7, "y": 8 }
            }))],
        )
        .unwrap();

    assert_eq!(report.edit_count, 1);
    assert!(report.added.is_empty(), "{report:?}");
    assert!(report.removed.is_empty(), "{report:?}");
    assert!(report.rebuilt.is_empty(), "{report:?}");
    assert!(report.controls_written.is_empty(), "{report:?}");
    assert_eq!(report.connections_added, 0);
    assert_eq!(report.connections_removed, 0);

    assert_eq!(position(&running, "osc"), Some((120.5, -40.0)));
    assert_eq!(position(&running, "aux"), Some((7.0, 8.0)));
    let saved = running.document_to_save().unwrap();
    assert_eq!(saved.layout.modules["osc"].x, 120.5);

    // Same modules and connections in the live graph.
    let after = running.full_snapshot();
    assert_eq!(before.modules.len(), after.modules.len());
    assert_eq!(before.connections.len(), after.connections.len());
    // The document's graph is untouched.
    let document = running.document().unwrap();
    assert_eq!(document.modules.len(), 4);
    assert_eq!(document.connections.len(), 2);
    running.stop();
}

/// Applies a batch while the pump keeps blocks flowing.
trait PumpedEdit {
    fn while_pumping_edit(
        &mut self,
        pump: &Pump,
        edits: &[StructuralEdit],
    ) -> Result<ApplyEditsReport, RpcError>;
}

impl PumpedEdit for RunningInvention {
    fn while_pumping_edit(
        &mut self,
        pump: &Pump,
        edits: &[StructuralEdit],
    ) -> Result<ApplyEditsReport, RpcError> {
        pump.while_pumping(|| self.apply_edits(edits))
    }
}

#[test]
fn c3_audio_continues_identically_to_an_unedited_twin() {
    let layout = layout_of(&[("osc", 1.0, 2.0)]);
    let (mut edited, edited_pump) = start(&document(Some(layout.clone())));
    let (twin, twin_pump) = start(&document(Some(layout)));

    edited_pump.render(200);
    twin_pump.render(200);

    let report = edited
        .while_pumping_edit(
            &edited_pump,
            &[move_edit(json!({
                "lfo": { "x": 10.0, "y": 10.0 },
                "osc": { "x": 120.5, "y": -40.0 },
                "dac": null
            }))],
        )
        .unwrap();
    assert!(report.rebuilt.is_empty() && report.added.is_empty() && report.removed.is_empty());

    edited_pump.render(300);
    twin_pump.render(300);

    // 500 blocks each were rendered by hand; the helper thread added more to
    // the edited run. The edit landed after at least 200 blocks.
    assert_same_audio(
        &edited_pump.recorded(),
        &twin_pump.recorded(),
        2 * BLOCK * 500,
    );
    assert_eq!(position(&edited, "osc"), Some((120.5, -40.0)));
    edited.stop();
    twin.stop();
}

/// Guards the twin comparison itself: it must be able to see a rebuild.
#[test]
fn c3_harness_notices_a_rebuilt_stateful_module() {
    let (mut edited, edited_pump) = start(&document(None));
    let (twin, twin_pump) = start(&document(None));
    edited_pump.render(200);
    twin_pump.render(200);

    edited
        .while_pumping_edit(
            &edited_pump,
            &[
                edit(json!({ "op": "remove_module", "id": "lfo" })),
                edit(json!({
                    "op": "add_module", "id": "lfo", "module_type": "lfo",
                    "config": { "rate": 3.0, "waveform": "sine" }
                })),
                edit(json!({
                    "op": "connect", "from": "lfo", "from_port": "bipolar",
                    "to": "osc", "to_port": "frequency"
                })),
            ],
        )
        .unwrap();
    edited_pump.render(300);
    twin_pump.render(300);

    let a = edited_pump.recorded();
    let b = twin_pump.recorded();
    let common = a.len().min(b.len());
    assert!(
        (0..common).any(|i| a[i].to_bits() != b[i].to_bits()),
        "a rebuilt lfo restarts its phase, so the audio must differ from the twin"
    );
    edited.stop();
    twin.stop();
}

#[test]
fn c3_null_clears_a_position_and_moves_are_absolute() {
    let (mut running, _pump) = start(&document(Some(layout_of(&[("osc", 1.0, 2.0)]))));
    let place = move_edit(json!({ "lfo": { "x": 5.0, "y": 6.0 } }));
    running.apply_edits(std::slice::from_ref(&place)).unwrap();
    running.apply_edits(std::slice::from_ref(&place)).unwrap();
    assert_eq!(position(&running, "lfo"), Some((5.0, 6.0)), "idempotent");

    running
        .apply_edits(&[move_edit(json!({ "osc": null }))])
        .unwrap();
    assert_eq!(position(&running, "osc"), None);
    assert_eq!(position(&running, "lfo"), Some((5.0, 6.0)));

    // Clearing a module that has no position is fine too.
    running
        .apply_edits(&[move_edit(json!({ "dac": null }))])
        .unwrap();
    assert_eq!(position(&running, "dac"), None);

    // The cleared module is gone from the saved file; clearing everything
    // leaves no layout key.
    running
        .apply_edits(&[move_edit(json!({ "lfo": null }))])
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let saved = saved_json(&running.document_to_save().unwrap(), dir.path());
    assert!(saved.get("layout").is_none(), "{saved}");
    running.stop();
}

#[test]
fn c3_later_moves_in_a_batch_win_and_see_earlier_edits() {
    let (mut running, _pump) = start(&document(None));
    let report = running
        .apply_edits(&[
            edit(json!({ "op": "add_module", "id": "extra", "module_type": "lfo", "config": {} })),
            move_edit(json!({ "extra": { "x": 1.0, "y": 1.0 } })),
            move_edit(json!({ "extra": { "x": 2.0, "y": 3.0 } })),
        ])
        .unwrap();
    assert_eq!(report.added, ["extra"]);
    assert_eq!(position(&running, "extra"), Some((2.0, 3.0)));
    running.stop();
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

fn assert_untouched_layout(running: &RunningInvention, expected: &[(&str, f64, f64)]) {
    let document = running.document().unwrap();
    assert_eq!(document.layout.modules.len(), expected.len());
    for &(id, x, y) in expected {
        let p = document.layout.modules.get(id).expect(id);
        assert_eq!((p.x, p.y), (x, y), "{id}");
    }
}

#[test]
fn refuse_unknown_module_as_an_invalid_edit() {
    let (mut running, _pump) = start(&document(Some(layout_of(&[("osc", 1.0, 2.0)]))));
    let error = running
        .apply_edits(&[move_edit(json!({ "ghost": { "x": 1.0, "y": 2.0 } }))])
        .unwrap_err();
    let failure = failure(&error);
    assert_eq!(failure.reason, EditFailureReason::UnknownModule);
    assert_eq!(failure.op, EditOp::MoveModules);
    assert_eq!(failure.index, 0);
    assert_untouched_layout(&running, &[("osc", 1.0, 2.0)]);
    running.stop();
}

#[test]
fn refuse_whole_batch_when_one_id_is_unknown() {
    let (mut running, _pump) = start(&document(Some(layout_of(&[("osc", 1.0, 2.0)]))));
    let error = running
        .apply_edits(&[
            move_edit(json!({ "osc": { "x": 50.0, "y": 60.0 } })),
            move_edit(json!({ "osc": { "x": 70.0, "y": 80.0 }, "ghost": { "x": 0.0, "y": 0.0 } })),
        ])
        .unwrap_err();
    let failure = failure(&error);
    assert_eq!(failure.reason, EditFailureReason::UnknownModule);
    assert_eq!(failure.index, 1);
    assert_untouched_layout(&running, &[("osc", 1.0, 2.0)]);
    running.stop();
}

#[test]
fn refuse_empty_positions_as_an_invalid_position() {
    let (mut running, _pump) = start(&document(None));
    let error = running.apply_edits(&[move_edit(json!({}))]).unwrap_err();
    let failure = failure(&error);
    assert_eq!(failure.reason, EditFailureReason::InvalidPosition);
    assert_eq!(failure.op, EditOp::MoveModules);
    running.stop();
}

#[test]
fn refuse_non_finite_coordinates_as_an_invalid_position() {
    let (mut running, _pump) = start(&document(Some(layout_of(&[("osc", 1.0, 2.0)]))));
    for (x, y) in [
        (f64::NAN, 0.0),
        (0.0, f64::NAN),
        (f64::INFINITY, 0.0),
        (0.0, f64::NEG_INFINITY),
    ] {
        let position = fugue::invention::Position { x, y };
        let positions = BTreeMap::from([("osc".to_string(), Some(position))]);
        let error = running
            .apply_edits(&[StructuralEdit::MoveModules { positions }])
            .unwrap_err();
        let failure = failure(&error);
        assert_eq!(
            failure.reason,
            EditFailureReason::InvalidPosition,
            "{x} {y}"
        );
        assert_untouched_layout(&running, &[("osc", 1.0, 2.0)]);
    }
    running.stop();
}

/// A document of `count` unconnected vcas, for the size limit.
fn many_modules(count: usize) -> String {
    let modules: Vec<Value> = (0..count)
        .map(|i| json!({ "id": format!("m{i}"), "type": "vca", "config": {} }))
        .collect();
    json!({ "version": "1.0.0", "modules": modules, "connections": [] }).to_string()
}

#[test]
fn limit_accepts_1024_positions_and_refuses_1025() {
    let (mut running, _pump) = start(&many_modules(MAX_MOVES_PER_EDIT + 1));

    let positions = |count: usize| -> Value {
        let map: serde_json::Map<String, Value> = (0..count)
            .map(|i| (format!("m{i}"), json!({ "x": i as f64, "y": -(i as f64) })))
            .collect();
        Value::Object(map)
    };

    let error = running
        .apply_edits(&[move_edit(positions(MAX_MOVES_PER_EDIT + 1))])
        .unwrap_err();
    // All 1025 ids exist, so the only fault is the count.
    assert_eq!(failure(&error).reason, EditFailureReason::InvalidPosition);
    assert!(layout_ids(&running).is_empty(), "refusal changed nothing");

    running
        .apply_edits(&[move_edit(positions(MAX_MOVES_PER_EDIT))])
        .unwrap();
    assert_eq!(layout_ids(&running).len(), MAX_MOVES_PER_EDIT);
    assert_eq!(position(&running, "m1023"), Some((1023.0, -1023.0)));
    assert_eq!(position(&running, "m1024"), None);
    running.stop();
}

#[test]
fn refuse_a_move_of_a_module_removed_earlier_in_the_batch() {
    let (mut running, _pump) = start(&document(Some(layout_of(&[("aux", 1.0, 2.0)]))));
    let error = running
        .apply_edits(&[
            edit(json!({ "op": "remove_module", "id": "aux" })),
            move_edit(json!({ "aux": { "x": 9.0, "y": 9.0 } })),
        ])
        .unwrap_err();
    let failure = failure(&error);
    assert_eq!(failure.reason, EditFailureReason::UnknownModule);
    assert_eq!(failure.index, 1);
    // Nothing applied: aux is still there with its position.
    assert_eq!(position(&running, "aux"), Some((1.0, 2.0)));
    assert!(running
        .document()
        .unwrap()
        .modules
        .iter()
        .any(|module| module.id == "aux"));
    running.stop();
}

#[test]
fn refuse_a_timed_batch_of_only_moves() {
    let (mut running, pump) = start(&document(Some(layout_of(&[("osc", 1.0, 2.0)]))));
    let result = pump.while_pumping(|| {
        running.apply_edits_at(
            &[move_edit(json!({ "osc": { "x": 9.0, "y": 9.0 } }))],
            &EditWhen::AfterSamples { samples: 64 * 4000 },
            None,
        )
    });
    match result {
        Err(_) => {}
        Ok(status) => assert_eq!(
            status.state,
            RequestState::Refused,
            "a timed batch of only moves must be refused, got {status:?}"
        ),
    }
    // Whatever the shape of the refusal, the move never lands.
    pump.render(5000);
    assert_untouched_layout(&running, &[("osc", 1.0, 2.0)]);
    running.stop();
}

// ---------------------------------------------------------------------------
// Criterion 4: removing a module removes its position
// ---------------------------------------------------------------------------

#[test]
fn c4_removing_a_module_by_edit_removes_its_position() {
    let layout = layout_of(&[("osc", 1.0, 2.0), ("aux", 3.0, 4.0)]);
    let (mut running, _pump) = start(&document(Some(layout)));
    running
        .apply_edits(&[edit(json!({ "op": "remove_module", "id": "aux" }))])
        .unwrap();
    assert_eq!(position(&running, "aux"), None);
    assert_eq!(position(&running, "osc"), Some((1.0, 2.0)));

    let dir = tempfile::tempdir().unwrap();
    let saved = saved_json(&running.document_to_save().unwrap(), dir.path());
    assert!(saved["layout"]["modules"].get("aux").is_none(), "{saved}");
    assert!(saved["layout"]["modules"].get("osc").is_some());
    running.stop();
}

#[test]
fn c4_a_re_added_module_does_not_inherit_the_old_position() {
    let layout = layout_of(&[("aux", 3.0, 4.0)]);
    let (mut running, _pump) = start(&document(Some(layout)));
    running
        .apply_edits(&[edit(json!({ "op": "remove_module", "id": "aux" }))])
        .unwrap();
    running
        .apply_edits(&[edit(json!({
            "op": "add_module", "id": "aux", "module_type": "lfo", "config": {}
        }))])
        .unwrap();
    assert_eq!(position(&running, "aux"), None);
    running.stop();
}

#[test]
fn c4_removing_a_module_after_moving_it_in_the_same_batch_leaves_no_position() {
    let (mut running, _pump) = start(&document(None));
    running
        .apply_edits(&[
            move_edit(json!({ "aux": { "x": 1.0, "y": 1.0 }, "osc": { "x": 2.0, "y": 2.0 } })),
            edit(json!({ "op": "remove_module", "id": "aux" })),
        ])
        .unwrap();
    assert_eq!(position(&running, "aux"), None);
    assert_eq!(position(&running, "osc"), Some((2.0, 2.0)));
    running.stop();
}

#[test]
fn c4_reloading_a_document_without_a_module_removes_its_position() {
    let layout = layout_of(&[("osc", 1.0, 2.0), ("aux", 3.0, 4.0)]);
    let (mut running, pump) = start(&document(Some(layout)));
    // The reloaded document no longer has aux, and says nothing of it.
    let reload =
        Invention::from_json(&document_without_aux(Some(layout_of(&[("osc", 1.0, 2.0)])))).unwrap();
    let report = pump.while_pumping(|| running.reload(reload)).unwrap();
    assert_eq!(report.removed, ["aux"], "{report:?}");
    assert_eq!(position(&running, "aux"), None);
    assert_eq!(position(&running, "osc"), Some((1.0, 2.0)));
    running.stop();
}

#[test]
fn c4_reloading_without_a_layout_section_clears_positions_of_removed_modules_only() {
    // A reloaded document is the new truth: its layout, absent here, is empty.
    let layout = layout_of(&[("osc", 1.0, 2.0), ("aux", 3.0, 4.0)]);
    let (mut running, pump) = start(&document(Some(layout)));
    let reload = Invention::from_json(&document_without_aux(None)).unwrap();
    pump.while_pumping(|| running.reload(reload)).unwrap();
    assert_eq!(position(&running, "aux"), None);
    running.stop();
}

// ---------------------------------------------------------------------------
// Criterion 5: unknown ids in a layout are dropped with a warning
// ---------------------------------------------------------------------------

fn ghost_layout() -> Value {
    layout_of(&[("osc", 1.0, 2.0), ("ghost", 9.0, 9.0)])
}

#[test]
fn c5_unknown_ids_in_a_layout_are_dropped_with_a_warning_on_load() {
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine
        .load_json(&document(Some(ghost_layout())))
        .expect("an unknown layout id is not a load failure");

    let warnings = engine.load_warnings();
    assert!(
        warnings.iter().any(|warning| warning.contains("ghost")),
        "a warning names the dropped id: {warnings:?}"
    );

    let retained = engine.document().unwrap();
    assert!(retained.layout.modules.contains_key("osc"));
    assert!(!retained.layout.modules.contains_key("ghost"));
    assert_eq!(retained.layout.modules.len(), 1);

    // It still plays.
    let mut out = vec![0.0f32; 2 * 4096];
    engine.render_interleaved(&mut out).unwrap();
    assert!(peak(&out) > 0.01);
}

#[test]
fn c5_a_dropped_id_is_not_saved() {
    let (running, _pump) = start(&document(Some(ghost_layout())));
    assert_eq!(layout_ids(&running), ["osc"]);
    let dir = tempfile::tempdir().unwrap();
    let saved = saved_json(&running.document_to_save().unwrap(), dir.path());
    assert!(saved["layout"]["modules"].get("ghost").is_none(), "{saved}");
    assert!(saved["layout"]["modules"].get("osc").is_some());
    running.stop();
}

#[test]
fn c5_a_reload_with_unknown_layout_ids_drops_them_with_a_warning() {
    let (mut running, pump) = start(&document(None));
    let reload = Invention::from_json(&document(Some(ghost_layout()))).unwrap();
    let report = pump
        .while_pumping(|| running.reload(reload))
        .expect("an unknown layout id is not a reload failure");
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("ghost")),
        "a warning names the dropped id: {:?}",
        report.warnings
    );
    assert_eq!(layout_ids(&running), ["osc"]);
    running.stop();
}

// ---------------------------------------------------------------------------
// Criterion 6: a layout-only reload rebuilds nothing
// ---------------------------------------------------------------------------

fn assert_nothing_rebuilt(report: &ReloadReport) {
    assert!(report.added.is_empty(), "{report:?}");
    assert!(report.removed.is_empty(), "{report:?}");
    assert!(report.swapped.is_empty(), "{report:?}");
    assert!(report.controls_updated.is_empty(), "{report:?}");
    assert!(report.controls_failed.is_empty(), "{report:?}");
    assert_eq!(report.connections_added, 0, "{report:?}");
    assert_eq!(report.connections_removed, 0, "{report:?}");
    assert_eq!(report.unchanged, 4, "{report:?}");
}

#[test]
fn c6_layout_only_reload_rebuilds_nothing_and_takes_the_new_layout() {
    let (mut edited, edited_pump) = start(&document(Some(layout_of(&[("osc", 1.0, 2.0)]))));
    let (twin, twin_pump) = start(&document(Some(layout_of(&[("osc", 1.0, 2.0)]))));
    edited_pump.render(200);
    twin_pump.render(200);

    let new_layout = layout_of(&[("osc", 77.5, -3.0), ("lfo", 0.25, 0.75)]);
    let reload = Invention::from_json(&document(Some(new_layout))).unwrap();
    let report = edited_pump.while_pumping(|| edited.reload(reload)).unwrap();
    assert_nothing_rebuilt(&report);

    assert_eq!(position(&edited, "osc"), Some((77.5, -3.0)));
    assert_eq!(position(&edited, "lfo"), Some((0.25, 0.75)));
    assert_eq!(layout_ids(&edited).len(), 2);
    let saved = edited.document_to_save().unwrap();
    assert_eq!(saved.layout.modules["osc"].x, 77.5);

    edited_pump.render(300);
    twin_pump.render(300);
    assert_same_audio(
        &edited_pump.recorded(),
        &twin_pump.recorded(),
        2 * BLOCK * 500,
    );
    edited.stop();
    twin.stop();
}

#[test]
fn c6_reload_that_drops_the_layout_rebuilds_nothing_and_clears_it() {
    let (mut running, pump) = start(&document(Some(layout_of(&[("osc", 1.0, 2.0)]))));
    let reload = Invention::from_json(&document(None)).unwrap();
    let report = pump.while_pumping(|| running.reload(reload)).unwrap();
    assert_nothing_rebuilt(&report);
    assert!(layout_ids(&running).is_empty());
    running.stop();
}

// ---------------------------------------------------------------------------
// Criterion 7: a move while a timed structural edit waits
// ---------------------------------------------------------------------------

/// Pumps blocks until request `id` is final (or too long passes), settling
/// it the way a host does.
fn run_until_final(running: &RunningInvention, pump: &Pump, id: &str) -> fugue::rpc::RequestStatus {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        pump.render(200);
        running.take_request_changes();
        if let Some(status) = running.request_status(id) {
            if status.state.is_final() {
                return status;
            }
            if status.state == RequestState::Landed {
                running.settle_requests();
            }
        }
        assert!(Instant::now() < deadline, "the timed edit never settled");
    }
}

#[test]
fn c7_a_move_while_a_timed_edit_waits_does_not_cancel_it_and_survives_its_landing() {
    let layout = layout_of(&[("osc", 1.0, 2.0), ("aux", 3.0, 4.0)]);
    let (mut running, pump) = start(&document(Some(layout)));
    pump.render(10);

    // The timed edit adds `extra` and removes `aux`; it lands far enough
    // ahead that the helper thread's few blocks cannot reach it.
    let queued = pump
        .while_pumping(|| {
            running.apply_edits_at(
                &[
                    edit(json!({
                        "op": "add_module", "id": "extra", "module_type": "lfo", "config": {}
                    })),
                    edit(json!({ "op": "remove_module", "id": "aux" })),
                ],
                &EditWhen::AfterSamples { samples: 64 * 6000 },
                None,
            )
        })
        .expect("the timed edit queues");
    assert_eq!(queued.state, RequestState::Queued, "{queued:?}");

    // While it waits: move osc, lfo, and aux (which the edit will remove).
    pump.while_pumping(|| {
        running.apply_edits(&[move_edit(json!({
            "osc": { "x": 10.5, "y": 20.5 },
            "lfo": { "x": -1.0, "y": -2.0 },
            "aux": { "x": 30.0, "y": 40.0 }
        }))])
    })
    .expect("a move is allowed while a timed edit waits");

    // The move is in the document at once, and the edit is still waiting.
    assert_eq!(position(&running, "osc"), Some((10.5, 20.5)));
    let waiting = running.request_status(&queued.id).expect("remembered");
    assert_eq!(
        waiting.state,
        RequestState::Queued,
        "a move must not cancel the waiting edit: {waiting:?}"
    );

    // Let it land.
    let settled = run_until_final(&running, &pump, &queued.id);
    assert_eq!(settled.state, RequestState::Adopted, "{settled:?}");

    let document = running.document().unwrap();
    let ids: Vec<&str> = document.modules.iter().map(|m| m.id.as_str()).collect();
    assert!(ids.contains(&"extra"), "{ids:?}");
    assert!(!ids.contains(&"aux"), "{ids:?}");
    // The move was not lost when the edit landed.
    assert_eq!(position(&running, "osc"), Some((10.5, 20.5)));
    assert_eq!(position(&running, "lfo"), Some((-1.0, -2.0)));
    // The removed module's position went with it; the added one has none.
    assert_eq!(position(&running, "aux"), None);
    assert_eq!(position(&running, "extra"), None);
    let saved = running.document_to_save().unwrap();
    assert_eq!(saved.layout.modules.len(), 2);
    running.stop();
}

#[test]
fn c7_a_timed_batch_may_carry_moves_beside_structure() {
    // Reading of "a timed batch of only moves is refused": a batch with
    // structure and a move is accepted, and its move lands with it.
    let (mut running, pump) = start(&document(Some(layout_of(&[("osc", 1.0, 2.0)]))));
    pump.render(10);
    let queued = pump
        .while_pumping(|| {
            running.apply_edits_at(
                &[
                    edit(json!({
                        "op": "add_module", "id": "extra", "module_type": "lfo", "config": {}
                    })),
                    move_edit(
                        json!({ "osc": { "x": 8.0, "y": 9.0 }, "extra": { "x": 1.5, "y": 2.5 } }),
                    ),
                ],
                &EditWhen::AfterSamples { samples: 64 * 3000 },
                None,
            )
        })
        .expect("structure plus a move queues");
    let settled = run_until_final(&running, &pump, &queued.id);
    assert_eq!(settled.state, RequestState::Adopted, "{settled:?}");
    assert_eq!(position(&running, "osc"), Some((8.0, 9.0)));
    assert_eq!(position(&running, "extra"), Some((1.5, 2.5)));
    running.stop();
}

#[test]
fn c7_a_move_does_not_disturb_the_audio_around_a_timed_edit() {
    // Twin plays the same timed edit with no move; the audio matches.
    let layout = layout_of(&[("osc", 1.0, 2.0)]);
    let timed = || {
        [edit(
            json!({ "op": "add_module", "id": "extra", "module_type": "lfo", "config": {} }),
        )]
    };
    let when = EditWhen::AfterSamples { samples: 64 * 3000 };

    let (mut moved, moved_pump) = start(&document(Some(layout.clone())));
    let (mut twin, twin_pump) = start(&document(Some(layout)));
    moved_pump.render(10);
    twin_pump.render(10);

    let moved_edit = moved_pump
        .while_pumping(|| moved.apply_edits_at(&timed(), &when, None))
        .unwrap();
    let twin_edit = twin_pump
        .while_pumping(|| twin.apply_edits_at(&timed(), &when, None))
        .unwrap();
    moved_pump
        .while_pumping(|| moved.apply_edits(&[move_edit(json!({ "osc": { "x": 5.0, "y": 5.0 } }))]))
        .unwrap();

    let moved_final = run_until_final(&moved, &moved_pump, &moved_edit.id);
    let twin_final = run_until_final(&twin, &twin_pump, &twin_edit.id);
    assert_eq!(moved_final.state, RequestState::Adopted, "{moved_final:?}");
    assert_eq!(twin_final.state, RequestState::Adopted, "{twin_final:?}");
    moved_pump.render(200);
    twin_pump.render(200);

    assert_same_audio(
        &moved_pump.recorded(),
        &twin_pump.recorded(),
        2 * BLOCK * 3000,
    );
    moved.stop();
    twin.stop();
}
