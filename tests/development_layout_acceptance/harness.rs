//! The rig: a whole invention running on the live engine through a capturing
//! `AudioBackend` (64-frame blocks rendered by hand, 48 kHz), the documents
//! the tests load, and small readers over the public snapshots.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use fugue::invention::{Layout, Position};
use fugue::rpc::{
    ApplyEditsReport, DevelopmentSnapshot, EditFailureReason, RuntimeRevision, StructuralEdit,
};
use fugue::{AudioBackend, Invention, InventionBuilder, RpcError, RpcErrorCode, RunningInvention};
use serde_json::{json, Value};

pub const RATE: u32 = 48_000;
const BLOCK: usize = 64;

type Render = Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>;

/// A backend that renders only when the test asks it to.
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

/// The packages directory every test in this binary resolves against, set
/// once before any piece starts so no test changes it under another.
pub fn packs_dir() -> &'static Path {
    static PACKS: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    let (_, path) = PACKS.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap().join("packs");
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var("FUGUE_PACKS_DIR", &path);
        (dir, path)
    });
    path
}

/// A running piece.
pub struct Piece {
    pub running: RunningInvention,
    /// What its load set aside without refusing it.
    pub load_warnings: Vec<String>,
    slot: Arc<Mutex<Option<Render>>>,
}

impl Piece {
    pub fn from_json(doc: &Value) -> Piece {
        let invention: Invention =
            serde_json::from_value(doc.clone()).unwrap_or_else(|e| panic!("parses: {e}\n{doc:#}"));
        Piece::start(invention)
    }

    pub fn from_file(path: &Path) -> Piece {
        let invention = Invention::from_file(path.to_str().unwrap())
            .unwrap_or_else(|e| panic!("loads {}: {e}", path.display()));
        Piece::start(invention)
    }

    fn start(invention: Invention) -> Piece {
        packs_dir();
        let (runtime, _) = InventionBuilder::new(RATE)
            .build(invention)
            .unwrap_or_else(|e| panic!("builds: {e}"));
        let load_warnings = runtime.load_warnings();
        let slot = Arc::new(Mutex::new(None));
        let running = runtime
            .start_with_backend(Capture(slot.clone()))
            .expect("a capturing backend starts");
        Piece {
            running,
            load_warnings,
            slot,
        }
    }

    /// Renders `blocks` blocks and returns both channels interleaved.
    pub fn render(&self, blocks: usize) -> Vec<f32> {
        let mut slot = self.slot.lock().unwrap();
        let render = slot.as_mut().expect("backend started");
        let (mut left, mut right) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
        let mut out = Vec::with_capacity(blocks * BLOCK * 2);
        for _ in 0..blocks {
            render(&mut left, &mut right);
            for (l, r) in left.iter().zip(right.iter()) {
                out.push(*l);
                out.push(*r);
            }
        }
        out
    }

    /// `get_development` for `path`.
    pub fn read(&self, path: &[&str]) -> Result<DevelopmentSnapshot, RpcError> {
        self.running.development(&names(path), revision())
    }

    pub fn inside(&self, path: &[&str]) -> DevelopmentSnapshot {
        self.read(path)
            .unwrap_or_else(|e| panic!("reads {path:?}: {e:?}"))
    }

    /// One `move_modules` inside the development at `path`, alone.
    pub fn arrange(
        &mut self,
        path: &[&str],
        positions: &[(&str, Option<(f64, f64)>)],
    ) -> Result<ApplyEditsReport, RpcError> {
        self.running.apply_edits(&[inside(path, positions)])
    }

    /// The retained authored document (`get_invention`).
    pub fn document(&self) -> Invention {
        self.running.document().expect("document retained")
    }

    pub fn module_count(&self) -> usize {
        self.running.full_snapshot().modules.len()
    }
}

pub fn revision() -> RuntimeRevision {
    RuntimeRevision {
        session_id: "acceptance".into(),
        revision: 1,
    }
}

pub fn names(path: &[&str]) -> Vec<String> {
    path.iter().map(|name| name.to_string()).collect()
}

fn positions(moves: &[(&str, Option<(f64, f64)>)]) -> BTreeMap<String, Option<Position>> {
    (moves.iter())
        .map(|(id, at)| (id.to_string(), at.map(|(x, y)| Position { x, y })))
        .collect()
}

/// A `move_modules` inside the development at `path`.
pub fn inside(path: &[&str], moves: &[(&str, Option<(f64, f64)>)]) -> StructuralEdit {
    StructuralEdit::MoveModules {
        development: Some(names(path)),
        positions: positions(moves),
    }
}

/// A `move_modules` of the piece's own modules, as before.
pub fn top_level(moves: &[(&str, Option<(f64, f64)>)]) -> StructuralEdit {
    StructuralEdit::MoveModules {
        development: None,
        positions: positions(moves),
    }
}

/// Where `layout` places `id`, if anywhere.
pub fn placed(layout: &Layout, id: &str) -> Option<(f64, f64)> {
    layout.modules.get(id).map(|p| (p.x, p.y))
}

/// Every placement of `layout`, as plain pairs.
pub fn placements(layout: &Layout) -> Vec<(String, (f64, f64))> {
    (layout.modules.iter())
        .map(|(id, p)| (id.clone(), (p.x, p.y)))
        .collect()
}

/// The inline definition reached by following `chain` of declaration names
/// from `document`.
pub fn definition<'a>(document: &'a Invention, chain: &[&str]) -> &'a Invention {
    let mut at = document;
    for name in chain {
        let spec = (at.developments.iter())
            .find(|spec| spec.name == *name)
            .unwrap_or_else(|| panic!("declares '{name}'"));
        at = spec.definition.as_deref().expect("an inline definition");
    }
    at
}

/// The index and reason of an `invalid_edit` refusal.
pub fn refused_edit(error: &RpcError) -> (usize, EditFailureReason) {
    assert_eq!(error.code, RpcErrorCode::InvalidEdit, "{error:?}");
    let edit = error.edit.as_ref().expect("an edit detail");
    (edit.index, edit.reason)
}

pub fn write_json(path: &Path, value: &Value) {
    write_text(path, &serde_json::to_string_pretty(value).unwrap());
}

pub fn write_text(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

pub fn read_text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// A temporary directory, canonical so paths compare as the daemon resolves
/// them.
pub fn temp() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap();
    (dir, path)
}

// -- documents ---------------------------------------------------------------

pub fn wire(from: &str, from_port: &str, to: &str, to_port: &str) -> Value {
    json!({ "from": from, "from_port": from_port, "to": to, "to_port": to_port })
}

/// A development of two sine oscillators `osc_a` and `osc_b` into a mixer
/// `mix`, out through `audio`.
pub fn voice(low: f64, high: f64) -> Value {
    json!({
        "version": "1.0.0",
        "modules": [
            { "id": "osc_a", "type": "oscillator",
              "config": { "waveform": "sine", "frequency": low } },
            { "id": "osc_b", "type": "oscillator",
              "config": { "waveform": "sine", "frequency": high } },
            { "id": "mix", "type": "mixer",
              "config": { "channel_count": 2, "levels": [0.3, 0.3] } },
        ],
        "connections": [
            wire("osc_a", "audio", "mix", "audio.0"),
            wire("osc_b", "audio", "mix", "audio.1"),
        ],
        "outputs": [ { "name": "audio", "from": "mix", "from_port": "audio_left" } ],
    })
}

/// A piece declaring `developments` and playing one instance of each type in
/// `played` (ids `p0`, `p1`, ...) mixed into the dac.
pub fn piece(developments: Value, played: &[&str]) -> Value {
    let mut modules: Vec<Value> = (played.iter().enumerate())
        .map(|(i, kind)| json!({ "id": format!("p{i}"), "type": kind }))
        .collect();
    let count = played.len();
    modules.push(json!({ "id": "bus", "type": "mixer",
        "config": { "channel_count": count, "levels": vec![0.5; count] } }));
    modules.push(json!({ "id": "dac", "type": "dac", "config": { "soft_clip": false } }));
    let mut connections: Vec<Value> = (0..count)
        .map(|i| wire(&format!("p{i}"), "audio", "bus", &format!("audio.{i}")))
        .collect();
    connections.push(wire("bus", "audio_left", "dac", "audio"));
    json!({
        "version": "1.0.0",
        "developments": developments,
        "modules": modules,
        "connections": connections,
    })
}
