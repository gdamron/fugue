//! A reload keeps the instances of developments that load no package, under
//! any policy: built with no lockfile and reloaded under one (what installing
//! a package does), built and reloaded with none, or under the same lockfile
//! found again. An identical document swaps nothing, the first reload
//! included, and plays on sample for sample; a changed definition rebuilds
//! exactly the instances that use it; and a policy change rebuilds only what
//! may hold package content.

use super::bytes::playable_kit;
use super::*;
use crate::invention::manual_backend::Settled;
use crate::invention::{InventionBuilder, ReloadReport};
use serde_json::Value;

const TONE: &str = "fugue.test.tone";

/// The policies a graph is built and then reloaded under.
#[derive(Clone, Copy, Debug)]
enum Policy {
    Ungoverned,
    SameLock,
    Changed,
}

const POLICIES: [Policy; 3] = [Policy::Ungoverned, Policy::SameLock, Policy::Changed];

fn wire(from: &str, from_port: &str, to: &str, to_port: &str) -> Value {
    json!({ "from": from, "from_port": from_port, "to": to, "to_port": to_port })
}

/// Two sines, `osc_a` and `osc_b`, into a mixer `mix`.
fn voice(low: f64, high: f64) -> Value {
    json!({
        "modules": [
            { "id": "osc_a", "type": "oscillator", "config": { "waveform": "sine", "frequency": low } },
            { "id": "osc_b", "type": "oscillator", "config": { "waveform": "sine", "frequency": high } },
            { "id": "mix", "type": "mixer", "config": { "channel_count": 2, "levels": [0.3, 0.3] } },
        ],
        "connections": [wire("osc_a", "audio", "mix", "audio.0"), wire("osc_b", "audio", "mix", "audio.1")],
        "outputs": [ { "name": "audio", "from": "mix", "from_port": "audio_left" } ],
    })
}

/// A development holding another, `inner`, played by one module `body`.
fn nested(low: f64) -> Value {
    json!({
        "developments": [ { "name": "inner", "definition": voice(low, 330.0) } ],
        "modules": [ { "id": "body", "type": "inner" } ],
        "connections": [],
        "outputs": [ { "name": "audio", "from": "body", "from_port": "audio" } ],
    })
}

/// A sine shaped by an envelope with a long release, gated through `gate`:
/// its stage is state a restart would lose.
fn enveloped(frequency: f64) -> Value {
    json!({
        "modules": [
            { "id": "osc", "type": "oscillator", "config": { "waveform": "sine", "frequency": frequency } },
            { "id": "env", "type": "adsr",
              "config": { "attack": 0.4, "decay": 0.3, "sustain": 0.5, "release": 1.5 } },
            { "id": "vca", "type": "vca" },
        ],
        "connections": [wire("osc", "audio", "vca", "audio"), wire("env", "envelope", "vca", "level")],
        "inputs": [ { "name": "gate", "to": "env", "to_port": "gate" } ],
        "outputs": [ { "name": "audio", "from": "vca", "from_port": "audio" } ],
    })
}

/// Installs `fugue.test.tone@version`, a development of [`nested`]`(low)`.
fn tone(packs: &Path, version: &str, low: f64) {
    let dir = packs.join(TONE).join(version);
    write(
        &dir.join("fugue.pkg.json"),
        json!({"id": TONE, "version": version, "kind": "development", "license": "MIT",
            "authors": [{"name": "Test"}], "targets": ["external-agent"],
            "entry": {"development": "voice.json"}})
        .to_string(),
    );
    write(&dir.join("voice.json"), nested(low).to_string());
}

/// A piece on disk playing an instance of each kind of development (two of
/// the inline one), with a clock gating the enveloped one, an unconnected
/// oscillator `side`, a `bus` and the `dac`.
struct Piece {
    dir: PathBuf,
    doc: Value,
    pad: Value,
}

impl Piece {
    fn new(dir: &Path) -> Self {
        let played = [
            "inline_a",
            "inline_b",
            "file_a",
            "package_a",
            "nested_a",
            "arranged_a",
            "env_a",
        ];
        let mut arranged = voice(150.0, 225.0);
        arranged["layout"] = json!({ "modules": {
            "osc_a": { "x": 10.0, "y": 20.0 }, "mix": { "x": 30.0, "y": 20.0 } } });
        let mut connections: Vec<Value> = (played.iter().enumerate())
            .map(|(i, id)| wire(id, "audio", "bus", &format!("audio.{i}")))
            .collect();
        connections.push(wire("clock", "beat", "env_a", "gate"));
        connections.push(wire("bus", "audio_left", "dac", "audio"));
        let doc = json!({
            "version": "1.0.0",
            "developments": [
                { "name": "voice", "definition": voice(220.0, 330.0) },
                { "name": "pad", "path": "pad.json" },
                { "name": "shared", "ref": { "package": TONE, "version": "1.0.0" } },
                { "name": "outer", "definition": nested(180.0) },
                { "name": "arranged", "definition": arranged },
                { "name": "enveloped", "definition": enveloped(330.0) },
            ],
            "modules": [
                { "id": "inline_a", "type": "voice" }, { "id": "inline_b", "type": "voice" },
                { "id": "file_a", "type": "pad" }, { "id": "package_a", "type": "shared" },
                { "id": "nested_a", "type": "outer" }, { "id": "arranged_a", "type": "arranged" },
                { "id": "env_a", "type": "enveloped" },
                { "id": "clock", "type": "clock", "config": { "bpm": 60.0 } },
                { "id": "side", "type": "oscillator", "config": { "frequency": 440.0 } },
                { "id": "bus", "type": "mixer", "config": { "channel_count": 7, "levels": vec![0.12; 7] } },
                { "id": "dac", "type": "dac", "config": { "soft_clip": false } },
            ],
            "connections": connections,
            "layout": { "overrides": {
                "shared/body": { "x": 1.0, "y": 2.0 }, "shared/inner/osc_a": { "x": 3.0, "y": 4.0 } } },
        });
        let piece = Self {
            dir: dir.to_path_buf(),
            doc,
            pad: voice(110.0, 165.0),
        };
        piece.write();
        piece
    }

    fn write(&self) {
        write(&self.dir.join("pad.json"), self.pad.to_string());
        write(&self.dir.join("piece.json"), self.doc.to_string());
    }

    fn load(&self) -> Invention {
        Invention::from_file(self.dir.join("piece.json").to_str().unwrap()).unwrap()
    }

    fn development(&mut self, name: &str) -> &mut Value {
        let declared = self.doc["developments"].as_array_mut().unwrap();
        (declared.iter_mut())
            .find(|spec| spec["name"] == name)
            .unwrap()
    }
}

/// Packages installed in a temp dir: the kit and `fugue.test.tone` 1.0.0 and
/// 2.0.0, with `body` given the dir, the packages dir and a lockfile locking
/// the kit and tone 1.0.0.
fn with_packages(body: impl FnOnce(&Path, &Path, &Path)) {
    let tmp = tempfile::tempdir().unwrap();
    let (packs, _) = playable_kit(tmp.path());
    tone(&packs, "1.0.0", 220.0);
    tone(&packs, "2.0.0", 240.0);
    let locked = lock(tmp.path(), &packs, &[(KIT, "1.0.0"), (TONE, "1.0.0")]);
    let lockfile = locked.path().to_path_buf();
    // A governed reload makes its lock the session's: `with_governing`
    // serializes that and gives the previous one back.
    with_packs_dir(&packs, || {
        with_governing(locked, || body(tmp.path(), &packs, &lockfile))
    });
}

/// The lockfile at `path`, found again as a host finds it for each load.
fn found(path: &Path, packs: &Path) -> Governance {
    Some(Arc::new(
        FrozenLock::find(&[path.to_path_buf()], packs)
            .unwrap()
            .unwrap(),
    ))
}

/// `invention` built under `policy`'s build side.
fn start(invention: Invention, policy: Policy, lockfile: &Path, packs: &Path) -> Settled {
    let build = match policy {
        Policy::SameLock => found(lockfile, packs),
        _ => None,
    };
    let (runtime, _) =
        govern_here(build, || InventionBuilder::new(48_000).build(invention)).unwrap();
    Settled::start(runtime)
}

/// A reload under `policy`'s reload side, its lockfile found again.
fn reload(
    running: &mut Settled,
    invention: Invention,
    policy: Policy,
    lockfile: &Path,
    packs: &Path,
) -> ReloadReport {
    match policy {
        Policy::Ungoverned => govern_here(None, || running.reload(invention)),
        _ => running.reload_governed(invention, found(lockfile, packs)),
    }
    .unwrap()
}

/// Both channels of `blocks` blocks, interleaved.
fn played(running: &Settled, blocks: usize) -> Vec<f32> {
    let (left, right) = running.pump.render_stereo(blocks);
    left.into_iter()
        .zip(right)
        .flat_map(|(l, r)| [l, r])
        .collect()
}

/// A reload that changed nothing: no module rebuilt, added or removed, no
/// control written, no wire touched, all 11 modules kept.
fn assert_nothing_changed(report: &ReloadReport, what: &str) {
    assert!(
        report.swapped.is_empty() && report.added.is_empty(),
        "{what}"
    );
    assert!(
        report.removed.is_empty() && report.controls_updated.is_empty(),
        "{what}"
    );
    assert_eq!(
        report.connections_added + report.connections_removed,
        0,
        "{what}"
    );
    assert_eq!(report.unchanged, 11, "{what}");
}

fn sorted(ids: &[String]) -> Vec<String> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids
}

#[test]
fn an_identical_reload_swaps_nothing_from_the_first_on_under_every_policy() {
    with_packages(|dir, packs, lockfile| {
        let piece = Piece::new(dir);
        for policy in POLICIES {
            let mut running = start(piece.load(), policy, lockfile, packs);
            for time in 1..=3 {
                let report = reload(&mut running, piece.load(), policy, lockfile, packs);
                assert_nothing_changed(&report, &format!("{policy:?}, reload {time}: {report:?}"));
            }
        }
    });
}

#[test]
fn a_reload_that_changes_nothing_a_development_plays_is_not_heard() {
    // Two copies play from one load and one is reloaded: a restarted
    // oscillator would be at phase zero, a restarted envelope idle.
    type Edit = fn(&mut Piece);
    let edits: [(&str, Edit); 3] = [
        ("identical", |_| {}),
        ("a scalar outside", |piece| {
            piece.doc["modules"][8]["config"]["frequency"] = json!(523.25)
        }),
        ("layout", |piece| {
            piece.doc["layout"]["modules"] = json!({
                "inline_a": { "x": 100.0, "y": 50.0 }, "bus": { "x": 400.0, "y": 80.0 } });
            piece.doc["layout"]["overrides"]["shared/body"] = json!({ "x": 9.0, "y": 9.0 });
        }),
    ];
    with_packages(|dir, packs, lockfile| {
        for (what, edit) in edits {
            for policy in POLICIES {
                let mut piece = Piece::new(dir);
                let reference = start(piece.load(), policy, lockfile, packs);
                let mut running = start(piece.load(), policy, lockfile, packs);
                let (mut heard_reference, mut heard) =
                    (played(&reference, 150), played(&running, 150));
                edit(&mut piece);
                piece.write();
                let report = reload(&mut running, piece.load(), policy, lockfile, packs);
                let said = format!("{what}, {policy:?}: {report:?}");
                assert!(report.swapped.is_empty(), "{said}");
                assert!(
                    report.added.is_empty() && report.removed.is_empty(),
                    "{said}"
                );
                match what {
                    "identical" => assert_nothing_changed(&report, &said),
                    "a scalar outside" => assert!(!report.controls_updated.is_empty(), "{said}"),
                    _ => {}
                }
                heard_reference.extend(played(&reference, 300));
                heard.extend(played(&running, 300));
                let peak =
                    |samples: &[f32]| samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
                assert!(
                    peak(&heard_reference[..150 * 128]) > 0.01,
                    "{what}: plays before"
                );
                assert!(
                    peak(&heard_reference[150 * 128..]) > 0.01,
                    "{what}: plays after"
                );
                assert_eq!(heard.len(), heard_reference.len(), "{what}: as much played");
                let differs = heard_reference
                    .iter()
                    .zip(&heard)
                    .position(|(a, b)| a.to_bits() != b.to_bits());
                assert_eq!(differs, None, "{what}, {policy:?}: the reload is heard");
            }
        }
    });
}

#[test]
fn a_changed_definition_rebuilds_exactly_the_instances_that_use_it() {
    type Edit = fn(&mut Piece);
    let edits: [(Edit, &[&str]); 7] = [
        (
            |p| p.development("voice")["definition"] = voice(233.0, 330.0),
            &["inline_a", "inline_b"],
        ),
        (|p| p.pad = voice(123.0, 165.0), &["file_a"]),
        (
            |p| p.development("outer")["definition"] = nested(190.0),
            &["nested_a"],
        ),
        (
            |p| {
                p.development("arranged")["definition"]["modules"][0]["config"]["frequency"] =
                    json!(160.0)
            },
            &["arranged_a"],
        ),
        (
            |p| p.development("enveloped")["definition"] = enveloped(345.0),
            &["env_a"],
        ),
        (
            |p| {
                p.development("voice")["definition"] = voice(233.0, 330.0);
                p.pad = voice(123.0, 165.0);
            },
            &["file_a", "inline_a", "inline_b"],
        ),
        (
            |p| p.doc["modules"][8] = json!({ "id": "side", "type": "lfo" }),
            &["side"],
        ),
    ];
    with_packages(|dir, packs, lockfile| {
        for (edit, expected) in edits {
            for policy in POLICIES {
                for before in [0, 1] {
                    let mut piece = Piece::new(dir);
                    let mut running = start(piece.load(), policy, lockfile, packs);
                    for _ in 0..before {
                        reload(&mut running, piece.load(), policy, lockfile, packs);
                    }
                    edit(&mut piece);
                    piece.write();
                    let report = reload(&mut running, piece.load(), policy, lockfile, packs);
                    let what = format!("{policy:?}, after {before}: {report:?}");
                    assert_eq!(sorted(&report.swapped), *expected, "{what}");
                    assert!(
                        report.added.is_empty() && report.removed.is_empty(),
                        "{what}"
                    );
                }
            }
        }
    });
}

#[test]
fn moving_a_package_reference_to_another_version_rebuilds_its_instance_only() {
    // Under a lockfile, the reload that resolves 2.0.0 is governed by one
    // locking 2.0.0, and the unchanged ones before it by one locking 1.0.0.
    with_packages(|dir, packs, lockfile| {
        let moved = tempfile::tempdir().unwrap();
        let newer = lock(moved.path(), packs, &[(KIT, "1.0.0"), (TONE, "2.0.0")]);
        for policy in [Policy::Ungoverned, Policy::Changed] {
            for before in [0, 1] {
                let mut piece = Piece::new(dir);
                let mut running = start(piece.load(), policy, lockfile, packs);
                for _ in 0..before {
                    let report = reload(&mut running, piece.load(), policy, lockfile, packs);
                    assert_nothing_changed(&report, &format!("unchanged: {report:?}"));
                }
                piece.development("shared")["ref"]["version"] = json!("2.0.0");
                piece.write();
                let report = reload(&mut running, piece.load(), policy, newer.path(), packs);
                assert_eq!(
                    report.swapped,
                    ["package_a"],
                    "{policy:?}, after {before}: {report:?}"
                );
            }
        }
    });
}

#[test]
fn a_policy_change_rebuilds_the_loaders_and_the_developments_holding_one_once() {
    let sampled = json!({
        "modules": [ { "id": "s", "type": "sample_player", "config": { "asset": format!("{KIT}@1.0.0:a.wav") } } ],
        "connections": [],
        "outputs": [ { "name": "audio", "from": "s", "from_port": "audio_left" } ],
    });
    let wrapper = json!({
        "developments": [ { "name": "sampled", "definition": sampled } ],
        "modules": [ { "id": "inner", "type": "sampled" } ],
        "connections": [],
        "outputs": [ { "name": "audio", "from": "inner", "from_port": "audio" } ],
    });
    let document: Invention = serde_json::from_value(json!({
        "developments": [
            { "name": "sampled", "definition": sampled },
            { "name": "wrapper", "definition": wrapper },
            { "name": "plain", "definition": nested(180.0) },
            { "name": "shared", "ref": { "package": TONE, "version": "1.0.0" } },
        ],
        "modules": [
            { "id": "loader", "type": "sample_player", "config": { "asset": format!("{KIT}@1.0.0:a.wav") } },
            { "id": "holds_loader", "type": "sampled" },
            { "id": "holds_nested_loader", "type": "wrapper" },
            { "id": "no_loader", "type": "plain" },
            { "id": "borrowed", "type": "shared" },
            { "id": "tone", "type": "oscillator", "config": { "frequency": 330.0 } },
            { "id": "bus", "type": "mixer", "config": { "channel_count": 2, "levels": [0.2, 0.2] } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } },
        ],
        "connections": [
            wire("no_loader", "audio", "bus", "audio.0"),
            wire("tone", "audio", "bus", "audio.1"),
            wire("bus", "audio_left", "dac", "audio"),
        ],
    }))
    .unwrap();
    with_packages(|_, packs, lockfile| {
        for policy in POLICIES {
            let mut running = start(document.clone(), policy, lockfile, packs);
            let rebuilt: Vec<_> = (0..3)
                .map(|_| {
                    sorted(&reload(&mut running, document.clone(), policy, lockfile, packs).swapped)
                })
                .collect();
            let first: &[&str] = match policy {
                Policy::Changed => &["holds_loader", "holds_nested_loader", "loader"],
                _ => &[],
            };
            assert_eq!(rebuilt[0], first, "{policy:?}");
            assert!(
                rebuilt[1].is_empty() && rebuilt[2].is_empty(),
                "{policy:?}: {rebuilt:?}"
            );
        }
    });
}
