//! A module panicking in a render: everything the render held stays with
//! the graph for teardown on a control thread, nothing is freed on the
//! rendering thread, every later submission is refused at once, and every
//! write still waiting is settled refused.
//!
//! The module panics with `resume_unwind`, which unwinds exactly as a
//! panic does but runs no panic hook, so the hook's printing does not
//! touch the allocator while the frees are counted.

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};

use super::super::announce::Progress;
use super::super::{EditState, EditTime};
use super::requests::try_submit_value;
use super::requests::{hook, submit};
use super::*;
use crate::alloc_counter::foreign_frees;
use crate::control_request::{
    ControlIndex, Outcome, Refusal, Request, RequestSender, RequestValue, When,
};
use crate::payload::{Payload, Retired};
use crate::{GraphModule, Module, ModuleBuildResult, ModuleFactory, MAX_BLOCK};

const FRAGILE: &str = "fragile";

/// What the fragile module panics in, once armed.
#[derive(Clone, Default)]
struct Armed {
    set_input: Arc<AtomicBool>,
    process: Arc<AtomicBool>,
    apply_payload: Arc<AtomicBool>,
}

/// Builds modules that pass their `level` input through and panic where
/// armed to.
#[derive(Clone, Default)]
struct FragileFactory(Armed);

impl ModuleFactory for FragileFactory {
    fn type_id(&self) -> &'static str {
        FRAGILE
    }

    fn build(
        &self,
        _sample_rate: u32,
        _config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(Fragile {
                armed: self.0.clone(),
                level: [0.0; MAX_BLOCK],
            })),
            handles: Vec::new(),
            control_surface: None,
            sink: None,
        })
    }
}

struct Fragile {
    armed: Armed,
    level: [f32; MAX_BLOCK],
}

/// The panic's payload: a unit, so making and catching it allocates nothing.
struct Unwound;

fn panic_if(armed: &AtomicBool) {
    if armed.load(Ordering::SeqCst) {
        resume_unwind(Box::new(Unwound));
    }
}

impl Module for Fragile {
    fn name(&self) -> &str {
        "Fragile"
    }

    fn process(&mut self, _frames: usize) -> bool {
        panic_if(&self.armed.process);
        true
    }

    fn inputs(&self) -> &[&str] {
        &["level"]
    }

    fn outputs(&self) -> &[&str] {
        &["out"]
    }

    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        &mut self.level
    }

    fn output_block(&self, _index: usize) -> &[f32] {
        &self.level
    }

    fn set_input(&mut self, _port: &str, value: f32) -> Result<(), String> {
        panic_if(&self.armed.set_input);
        self.level.fill(value);
        Ok(())
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        Err(format!("no output '{port}'"))
    }

    fn apply_payload(
        &mut self,
        _control: ControlIndex,
        payload: Payload,
    ) -> Result<Retired, (Refusal, Payload)> {
        panic_if(&self.armed.apply_payload);
        Err((Refusal::Unsupported, payload))
    }
}

/// A fragile module and an oscillator into the dac, its requests applied
/// as input writes (its `set_input`).
fn fragile_rig() -> (Rig, Armed) {
    let mut rig = Rig::new(BASE);
    let factory = FragileFactory::default();
    let armed = factory.0.clone();
    rig.registry.register(factory);
    let fragile = rig.build("fragile", FRAGILE, serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("fragile", fragile);
            change.connect(edge("fragile", "out", "dac", "audio"))
        })
        .unwrap();
    rig.render(1);
    hook(&mut rig);
    (rig, armed)
}

/// A queued edit whose payload holds a sender into the queue it waits in,
/// as an edit's modules hold their controls' senders: until the queue drops
/// it, it keeps the queue alive, and so itself.
struct Cycle {
    _sender: RequestSender,
    dropped: Arc<AtomicBool>,
}

impl Drop for Cycle {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

/// Queues a [`Cycle`] edit behind everything submitted so far.
fn queue_cycle(rig: &Rig) -> Arc<AtomicBool> {
    let dropped = Arc::new(AtomicBool::new(false));
    let cycle = Cycle {
        _sender: rig.live.requests.clone(),
        dropped: Arc::clone(&dropped),
    };
    let publisher = rig.live.publisher().lock().unwrap();
    let edit = Request::edit(publisher.generation(), Payload::owned(Box::new(cycle)));
    rig.live.requests.submit(edit).ok().unwrap();
    dropped
}

/// Renders one block, which must panic, counting the frees it makes of
/// memory allocated before it.
fn panicking_block(rig: &mut Rig) -> usize {
    let (rendered, foreign) = foreign_frees(|| catch_unwind(AssertUnwindSafe(|| rig.render(1))));
    let panic = rendered.expect_err("the module panics");
    assert!(panic.is::<Unwound>());
    foreign
}

/// Every way in is refused at once, and the graph renders silence.
fn assert_refused(rig: &mut Rig) {
    assert!(!rig.live.publisher().lock().unwrap().audio_alive());
    assert!(matches!(
        rig.live.write_input("fragile", "level", 0.5),
        Err(GraphCommandError::AudioThreadStopped)
    ));
    let built = rig.build("osc3", "oscillator", serde_json::json!({}));
    let edited = rig.live.edit(|change| {
        change.upsert("osc3", built);
        Ok(())
    });
    assert!(matches!(edited, Err(GraphCommandError::AudioThreadStopped)));
    assert!(rig.render(2).iter().all(|sample| *sample == 0.0));
}

/// A request applying at a block start, ahead of an edit popped to install
/// after it, panics: the drain, the link and the popped edit stay with the
/// graph, and the cycle left in the queue is freed with it.
#[test]
fn a_panic_ahead_of_an_installing_edit_frees_nothing_and_leaks_nothing() {
    let (mut rig, armed) = fragile_rig();
    let start = rig.graph.current_sample;
    // One waiting, one applying now.
    submit(&rig, "fragile", 0, 0.25, When::AtSample(start + 64 * 100));
    submit(&rig, "fragile", 0, 0.5, When::Now);
    let osc = rig.build("osc3", "oscillator", serde_json::json!({}));
    rig.publish_unreclaimed(|change| change.upsert("osc3", osc));
    let cycle = queue_cycle(&rig);
    armed.set_input.store(true, Ordering::SeqCst);

    assert_eq!(
        panicking_block(&mut rig),
        0,
        "freed on the rendering thread"
    );
    assert!(rig.graph.requests.is_some() && rig.graph.link.is_some());
    assert_refused(&mut rig);

    let Rig { graph, live, .. } = rig;
    drop(graph);
    drop(live);
    assert!(cycle.load(Ordering::SeqCst), "the queue leaked");
}

/// A request applying mid-block panics, at a segment start with the link
/// in the graph: the drain stays with it, and the queue is closed.
#[test]
fn a_panic_applying_a_request_mid_block_refuses_later_submissions() {
    let (mut rig, armed) = fragile_rig();
    let start = rig.graph.current_sample;
    submit(&rig, "fragile", 0, 0.25, When::AtSample(start + 64 * 100));
    submit(&rig, "fragile", 0, 0.5, When::AtSample(start + 10));
    armed.set_input.store(true, Ordering::SeqCst);

    assert_eq!(
        panicking_block(&mut rig),
        0,
        "freed on the rendering thread"
    );
    assert!(rig.graph.requests.is_some());
    assert_refused(&mut rig);
}

/// An input write applying as an edit installs panics, with the edit's
/// publication in hand: it stays with the graph too.
#[test]
fn a_panic_as_an_edit_installs_keeps_its_publication() {
    let (mut rig, armed) = fragile_rig();
    rig.live.write_input("fragile", "level", 0.5).unwrap();
    let osc = rig.build("osc3", "oscillator", serde_json::json!({}));
    rig.publish_unreclaimed(|change| change.upsert("osc3", osc));
    armed.set_input.store(true, Ordering::SeqCst);

    assert_eq!(
        panicking_block(&mut rig),
        0,
        "freed on the rendering thread"
    );
    assert!(rig.graph.link.is_some());
    assert_refused(&mut rig);
}

/// A module panicking as it processes, with nothing taken out of the graph,
/// fails the graph all the same.
#[test]
fn a_panic_in_process_refuses_later_submissions() {
    let (mut rig, armed) = fragile_rig();
    armed.process.store(true, Ordering::SeqCst);

    assert_eq!(
        panicking_block(&mut rig),
        0,
        "freed on the rendering thread"
    );
    assert_refused(&mut rig);
}

/// A payload that says when it is freed.
struct Flagged(Arc<AtomicBool>);

impl Drop for Flagged {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// A module panics in `apply_payload` with the payload it was handed: the
/// unwind sets it aside rather than freeing it on the rendering thread, and
/// the graph keeps it until its teardown frees it.
#[test]
fn a_panic_applying_a_payload_keeps_it_for_teardown() {
    let (mut rig, armed) = fragile_rig();
    rig.graph.request_hook = None;
    let freed = Arc::new(AtomicBool::new(false));
    let payload = Payload::owned(Box::new(Flagged(Arc::clone(&freed))));
    let request = RequestValue::Payload(payload);
    try_submit_value(&rig, "fragile", 0, request, When::Now).unwrap();
    armed.apply_payload.store(true, Ordering::SeqCst);

    assert_eq!(
        panicking_block(&mut rig),
        0,
        "freed on the rendering thread"
    );
    assert_eq!(rig.graph.caught.iter().flatten().count(), 1, "kept");
    assert_refused(&mut rig);
    assert!(!freed.load(Ordering::SeqCst));

    let Rig { graph, live, .. } = rig;
    drop(graph);
    drop(live);
    assert!(freed.load(Ordering::SeqCst), "the payload leaked");
}

/// Writes waiting as a render panics, one due in the panicking block and
/// one later, are settled refused (`Refusal::Stopped`): neither is listed
/// as waiting until the invention stops.
#[test]
fn writes_waiting_as_a_render_panics_settle_refused() {
    let (mut rig, armed) = fragile_rig();
    let start = rig.graph.current_sample;
    let later = When::AtSample(start + 64 * 100);
    let waiting = submit(&rig, "fragile", 0, 0.25, later);
    let applying = submit(&rig, "fragile", 0, 0.5, When::AtSample(start + 10));
    for id in [waiting, applying] {
        let write = super::lost_outcomes::write(0.5, later);
        rig.live.pending.lock().unwrap().submitted(id, write);
    }
    armed.set_input.store(true, Ordering::SeqCst);

    assert_eq!(
        panicking_block(&mut rig),
        0,
        "freed on the rendering thread"
    );
    let stopped = Progress::Settled(Outcome::Refused(Refusal::Stopped));
    assert_eq!(rig.live.progress(waiting), stopped);
    assert_eq!(rig.live.progress(applying), stopped);
    assert!(rig.live.pending_requests().is_empty());
}

/// A timed edit waiting as untimed outcomes are lost, then as a render
/// panics, is settled refused (`Refusal::Stopped`): whether the loss was
/// taken before the panic (clearing the log's edits, the waiting one
/// left to the publisher) or with it.
#[test]
fn a_timed_edit_waiting_past_lost_outcomes_settles_stopped_as_a_render_panics() {
    for taken_before in [true, false] {
        let (mut rig, armed) = fragile_rig();
        let far = EditTime::AtSample(rig.graph.current_sample + 48_000 * 60);
        let mut change = rig.live.begin();
        change.remove("fragile");
        let prepared = change.prepare().unwrap();
        let id = rig.live.commit_when(prepared, far, None, |_| {}).unwrap();
        // More untimed outcomes than the queue holds, none received.
        for _ in 0..8 {
            for value in 0..200 {
                submit(&rig, "fragile", 0, value as f32, When::Now);
            }
            rig.render(8);
        }
        if taken_before {
            let status = rig.live.timed_status(id).unwrap();
            assert_eq!(status.state, EditState::Queued, "the waiting edit stays");
        }
        armed.set_input.store(true, Ordering::SeqCst);
        submit(&rig, "fragile", 0, 0.5, When::Now);
        panicking_block(&mut rig);

        let stopped = EditState::Refused(Refusal::Stopped);
        assert_eq!(rig.live.timed_status(id).unwrap().state, stopped);
    }
}
