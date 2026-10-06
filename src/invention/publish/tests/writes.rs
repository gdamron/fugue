//! Input writes that race a publication: each lands on the instance it was
//! resolved against, waits for a publication not yet installed, or is
//! dropped when its target went away. A probe module records every write it
//! receives with the instance that received it.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::{GraphModule, Module, ModuleBuildResult, ModuleFactory, MAX_BLOCK};

const WRITE_PROBE: &str = "write_probe";

/// One write a probe received: the probe's id (from its config), the
/// instance's build serial, the port, and the value.
type Received = (String, usize, String, f32);

/// Builds probes that log every input write they receive.
#[derive(Clone, Default)]
struct WriteProbeFactory {
    log: Arc<Mutex<Vec<Received>>>,
    built: Arc<AtomicUsize>,
}

impl WriteProbeFactory {
    /// Takes the writes received so far, in the order they arrived.
    fn take(&self) -> Vec<Received> {
        std::mem::take(&mut *self.log.lock().unwrap())
    }
}

impl ModuleFactory for WriteProbeFactory {
    fn type_id(&self) -> &'static str {
        WRITE_PROBE
    }

    fn build(
        &self,
        _sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(WriteProbe {
                id: config["id"].as_str().unwrap_or_default().to_string(),
                serial: self.built.fetch_add(1, Ordering::Relaxed),
                log: self.log.clone(),
                inputs: [[0.0; MAX_BLOCK]; 2],
                output: [0.0; MAX_BLOCK],
            })),
            handles: Vec::new(),
            control_surface: None,
            sink: None,
        })
    }
}

struct WriteProbe {
    id: String,
    serial: usize,
    log: Arc<Mutex<Vec<Received>>>,
    inputs: [[f32; MAX_BLOCK]; 2],
    output: [f32; MAX_BLOCK],
}

impl Module for WriteProbe {
    fn name(&self) -> &str {
        "WriteProbe"
    }

    fn process(&mut self, _frames: usize) -> bool {
        true
    }

    fn inputs(&self) -> &[&str] {
        &["a", "b"]
    }

    fn outputs(&self) -> &[&str] {
        &["out"]
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        &mut self.inputs[index]
    }

    fn output_block(&self, _index: usize) -> &[f32] {
        &self.output
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        let entry = (self.id.clone(), self.serial, port.to_string(), value);
        self.log.lock().unwrap().push(entry);
        Ok(())
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        Err(format!("no output '{port}'"))
    }
}

fn got(id: &str, serial: usize, port: &str, value: f32) -> Received {
    (id.to_string(), serial, port.to_string(), value)
}

/// The base rig with a probe for each of `ids` appended (built in order,
/// so probe `ids[n]` is serial `n`), installed.
fn rig_with_probes(ids: &[&str]) -> (Rig, WriteProbeFactory) {
    let probes = WriteProbeFactory::default();
    let mut rig = Rig::new(BASE);
    rig.registry.register(probes.clone());
    for id in ids {
        let probe = probe(&rig, id);
        rig.live
            .edit(|change| {
                change.upsert(id, probe);
                Ok(())
            })
            .unwrap();
    }
    rig.render(1);
    assert_mirror_matches(&rig);
    (rig, probes)
}

fn probe(rig: &Rig, id: &str) -> change::BuiltModule {
    rig.build(id, WRITE_PROBE, serde_json::json!({ "id": id }))
}

/// The publisher's mirror, which writes are resolved against, lists the
/// modules in the order the audio graph runs them.
fn assert_mirror_matches(rig: &Rig) {
    let mirror: Vec<String> = {
        let publisher = rig.live.publisher().lock().unwrap();
        publisher.mirror().modules.keys().cloned().collect()
    };
    assert_eq!(mirror, rig.module_ids());
}

#[test]
fn a_write_follows_its_module_to_a_new_index() {
    let (mut rig, probes) = rig_with_probes(&["p1", "p2"]);
    // Resolved with p1 at index 3; removing osc1 moves p1 to 2 and puts p2
    // at 3 before the audio thread runs.
    rig.live.write_input("p1", "b", 1.0).unwrap();
    rig.live.remove_module("osc1").unwrap();
    rig.render(1);

    assert_eq!(rig.module_ids(), ["osc2", "dac", "p1", "p2"]);
    assert_mirror_matches(&rig);
    assert_eq!(probes.take(), [got("p1", 0, "b", 1.0)]);
}

#[test]
fn a_write_for_a_pending_publication_waits_for_its_install() {
    let (mut rig, probes) = rig_with_probes(&["p1", "p2"]);
    rig.hold_a_retirement();
    // Removes osc1 and adds p3, but stays untaken while a retirement is
    // held. Against the running graph p2's new index 3 is p1, and p3's
    // new index 4 is p2.
    let p3 = probe(&rig, "p3");
    rig.publish_unreclaimed(|change| {
        change.remove("osc1");
        change.upsert("p3", p3);
    });
    rig.live.write_input("p2", "a", 2.0).unwrap();
    rig.live.write_input("p3", "b", 3.0).unwrap();

    // Blocks run on the old graph; the writes wait rather than land on
    // whatever holds their indices there.
    rig.render(2);
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac", "p1", "p2"]);
    assert_eq!(probes.take(), []);

    // The publication installs, and the writes follow in order.
    rig.live.reclaim();
    rig.render(1);
    assert_eq!(rig.module_ids(), ["osc2", "dac", "p1", "p2", "p3"]);
    assert_mirror_matches(&rig);
    assert_eq!(
        probes.take(),
        [got("p2", 1, "a", 2.0), got("p3", 2, "b", 3.0)]
    );
}

#[test]
fn a_write_to_a_removed_or_rebuilt_module_is_dropped() {
    let (mut rig, probes) = rig_with_probes(&["p1", "p2"]);
    rig.live.write_input("p1", "a", 1.0).unwrap();
    rig.live.write_input("p2", "a", 2.0).unwrap();
    // Rebuilds p1 (serial 2) and removes p2 before the audio thread runs.
    let rebuilt = probe(&rig, "p1");
    rig.live
        .edit(|change| {
            change.upsert("p1", rebuilt);
            change.remove("p2");
            Ok(())
        })
        .unwrap();
    rig.render(1);

    // Neither target exists any more; the replacement gets nothing.
    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac", "p1"]);
    assert_eq!(probes.take(), []);
    rig.live.write_input("p1", "a", 3.0).unwrap();
    rig.render(1);
    assert_eq!(probes.take(), [got("p1", 2, "a", 3.0)]);
}

/// Publishes one change under the publisher, as a script's edit would.
fn edit(rig: &Rig, apply: impl FnOnce(&mut GraphChange)) {
    rig.live
        .edit(|change| {
            apply(change);
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_write_for_a_folded_publication_reaches_its_instance() {
    let (mut rig, probes) = rig_with_probes(&["p1"]);
    // Against the running graph (generation 1).
    rig.live.write_input("p1", "a", 1.0).unwrap();
    // Generation 2 adds p2; these resolve against it.
    let p2 = probe(&rig, "p2");
    edit(&rig, |change| change.upsert("p2", p2));
    rig.live.write_input("p2", "a", 2.0).unwrap();
    rig.live.write_input("p1", "a", 3.0).unwrap();
    // Generation 3 removes osc1 and absorbs the untaken generation 2.
    edit(&rig, |change| change.remove("osc1"));
    rig.live.write_input("p2", "a", 4.0).unwrap();
    rig.render(1);

    // Every write follows its module into the installed order, not its
    // stale index (p1's index 3 in generation 2 is p2's now).
    assert_eq!(rig.module_ids(), ["osc2", "dac", "p1", "p2"]);
    assert_mirror_matches(&rig);
    assert_eq!(
        probes.take(),
        [
            got("p1", 0, "a", 1.0),
            got("p2", 1, "a", 2.0),
            got("p1", 0, "a", 3.0),
            got("p2", 1, "a", 4.0),
        ]
    );
}

#[test]
fn writes_follow_their_modules_through_two_folds() {
    let (mut rig, probes) = rig_with_probes(&["p1"]);
    rig.live.write_input("p1", "a", 1.0).unwrap();
    let p2 = probe(&rig, "p2");
    edit(&rig, |change| change.upsert("p2", p2));
    rig.live.write_input("p2", "a", 2.0).unwrap();
    // Generation 3 folds 2 in; generation 4 folds 3 (carrying 2) in.
    let p3 = probe(&rig, "p3");
    edit(&rig, |change| change.upsert("p3", p3));
    rig.live.write_input("p3", "b", 3.0).unwrap();
    rig.live.write_input("p2", "b", 4.0).unwrap();
    edit(&rig, |change| change.remove("osc1"));
    rig.live.write_input("p2", "a", 5.0).unwrap();
    rig.render(1);

    assert_eq!(rig.module_ids(), ["osc2", "dac", "p1", "p2", "p3"]);
    assert_eq!(
        probes.take(),
        [
            got("p1", 0, "a", 1.0),
            got("p2", 1, "a", 2.0),
            got("p3", 2, "b", 3.0),
            got("p2", 1, "b", 4.0),
            got("p2", 1, "a", 5.0),
        ]
    );
}

#[test]
fn a_folded_write_to_a_module_rebuilt_by_the_fold_is_dropped() {
    let (mut rig, probes) = rig_with_probes(&["p1"]);
    let p2 = probe(&rig, "p2");
    edit(&rig, |change| change.upsert("p2", p2));
    rig.live.write_input("p2", "a", 1.0).unwrap();
    // Generation 3 replaces the p2 generation 2 built, before either runs.
    let rebuilt = probe(&rig, "p2");
    edit(&rig, |change| change.upsert("p2", rebuilt));
    rig.render(1);

    assert_eq!(rig.module_ids(), ["osc1", "osc2", "dac", "p1", "p2"]);
    assert_eq!(probes.take(), []);
}

#[test]
fn writes_for_the_running_and_a_pending_generation_go_their_own_ways() {
    let (mut rig, probes) = rig_with_probes(&["p1"]);
    rig.hold_a_retirement();
    rig.live.write_input("p1", "a", 1.0).unwrap();
    let p2 = probe(&rig, "p2");
    rig.publish_unreclaimed(|change| change.upsert("p2", p2));
    rig.live.write_input("p2", "a", 2.0).unwrap();

    // The running graph's write applies now; the pending one waits.
    rig.render(1);
    assert_eq!(probes.take(), [got("p1", 0, "a", 1.0)]);
    rig.live.reclaim();
    rig.render(1);
    assert_eq!(probes.take(), [got("p2", 1, "a", 2.0)]);
}

#[test]
fn a_full_ring_leaves_the_queue_full_and_loses_nothing() {
    let (mut rig, probes) = rig_with_probes(&["p1"]);
    rig.hold_a_retirement();
    let p2 = probe(&rig, "p2");
    rig.publish_unreclaimed(|change| change.upsert("p2", p2));
    let capacity = publisher::INPUT_QUEUE_CAPACITY;
    let mut next = 0;
    let mut fill = |rig: &Rig| {
        for _ in 0..capacity {
            rig.live.write_input("p2", "b", next as f32).unwrap();
            next += 1;
        }
    };
    let refused = |rig: &Rig| {
        matches!(
            rig.live.write_input("p2", "b", -1.0),
            Err(GraphCommandError::QueueFull)
        )
    };

    // The queue fills; a block moves it into the ring; it fills again.
    fill(&rig);
    assert!(refused(&rig));
    rig.render(1);
    fill(&rig);
    assert!(refused(&rig));
    // With the ring full the channel is left alone: still refused.
    rig.render(1);
    assert!(refused(&rig));
    assert_eq!(probes.take(), []);

    // The publication installs and every write lands, in order.
    rig.live.reclaim();
    rig.render(1);
    let values: Vec<f32> = (0..2 * capacity).map(|n| n as f32).collect();
    let received = probes.take();
    assert!(received
        .iter()
        .all(|(id, serial, port, _)| { (id.as_str(), *serial, port.as_str()) == ("p2", 1, "b") }));
    assert_eq!(
        received
            .iter()
            .map(|(.., value)| *value)
            .collect::<Vec<_>>(),
        values
    );
    rig.live.write_input("p2", "b", 0.0).unwrap();
}

#[test]
fn unknown_modules_and_ports_are_refused_at_once() {
    let (rig, _probes) = rig_with_probes(&["p1"]);
    assert!(matches!(
        rig.live.write_input("missing", "a", 0.0),
        Err(GraphCommandError::UnknownModule(id)) if id == "missing"
    ));
    assert!(matches!(
        rig.live.write_input("p1", "out", 0.0),
        Err(GraphCommandError::InvalidPort(_))
    ));
}
