//! Control requests on a live graph: each applies exactly at its sample, by
//! splitting the block, and the drain never touches the allocator. The test
//! hook applies a request as an input write, with the control index as the
//! input port's index.

use super::*;
use crate::control_request::{
    ControlIndex, Outcome, QueueFull, Refusal, Request, RequestId, RequestValue, RtValue, When,
};
use crate::{GraphModule, Module, ModuleBuildResult, ModuleFactory, MAX_BLOCK};

/// Submits a request as a front door would: resolved and queued under the
/// publisher, then noted, unless the queue was full.
pub(super) fn try_submit(
    rig: &Rig,
    module_id: &str,
    control: u16,
    value: f32,
    when: When,
) -> Result<RequestId, QueueFull> {
    let value = RequestValue::Value(RtValue::F32(value));
    try_submit_value(rig, module_id, control, value, when)
}

/// [`try_submit`] for any value, a payload included.
pub(super) fn try_submit_value(
    rig: &Rig,
    module_id: &str,
    control: u16,
    value: RequestValue,
    when: When,
) -> Result<RequestId, QueueFull> {
    let mut publisher = rig.live.publisher().lock().unwrap();
    let target = publisher
        .control_target(module_id, ControlIndex(control))
        .unwrap();
    let mut request = Request::new(target, value);
    request.when = when;
    let id = rig.live.requests.submit(request)?;
    publisher.note_written();
    Ok(id)
}

/// [`try_submit`], which must find room.
pub(super) fn submit(
    rig: &Rig,
    module_id: &str,
    control: u16,
    value: f32,
    when: When,
) -> RequestId {
    try_submit(rig, module_id, control, value, when).unwrap()
}

/// Makes `rig`'s graph apply requests as input writes.
pub(super) fn hook(rig: &mut Rig) {
    rig.graph.request_hook = Some(SignalGraph::request_as_input_write);
}

/// Receives the outcomes settled so far, as a front door would.
pub(super) fn outcomes(rig: &mut Rig) -> Vec<(RequestId, Outcome)> {
    rig.live.take_outcomes()
}

const LEVEL: &str = "level";

/// Builds modules whose output is their `level` input, held.
#[derive(Clone)]
struct LevelFactory;

impl ModuleFactory for LevelFactory {
    fn type_id(&self) -> &'static str {
        LEVEL
    }

    fn build(
        &self,
        _sample_rate: u32,
        _config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(Level {
                level: [0.0; MAX_BLOCK],
                out: [0.0; MAX_BLOCK],
            })),
            handles: Vec::new(),
            control_surface: None,
            sink: None,
        })
    }
}

struct Level {
    level: [f32; MAX_BLOCK],
    out: [f32; MAX_BLOCK],
}

impl Module for Level {
    fn name(&self) -> &str {
        "Level"
    }

    fn process(&mut self, frames: usize) -> bool {
        self.out[..frames].copy_from_slice(&self.level[..frames]);
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
        &self.out
    }

    fn set_input(&mut self, _port: &str, value: f32) -> Result<(), String> {
        self.level.fill(value);
        Ok(())
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        Err(format!("no output '{port}'"))
    }
}

/// A level module alone into an unclipped dac, so the output is the level.
pub(super) fn level_rig() -> Rig {
    let mut rig = Rig::new(
        r#"{
            "version": "1.0.0",
            "modules": [{ "id": "dac", "type": "dac", "config": { "soft_clip": false } }],
            "connections": []
        }"#,
    );
    rig.registry.register(LevelFactory);
    let level = rig.build("level", LEVEL, serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("level", level);
            change.connect(edge("level", "out", "dac", "audio"))
        })
        .unwrap();
    rig.render(1);
    hook(&mut rig);
    rig
}

#[test]
fn a_timed_request_applies_exactly_at_its_sample() {
    let mut rig = level_rig();
    let start = rig.graph.current_sample;
    // 21 samples into the second block, then one at the first's start.
    let at = start + 64 + 21;
    let late = submit(&rig, "level", 0, 0.5, When::AtSample(at));
    let now = submit(&rig, "level", 0, 0.25, When::Now);

    let out = rig.render(2);
    assert!(out[..85].iter().all(|v| *v == 0.25), "{out:?}");
    assert!(out[85..].iter().all(|v| *v == 0.5), "{out:?}");
    assert_eq!(
        outcomes(&mut rig),
        [
            (now, Outcome::Applied { at: start }),
            (late, Outcome::Applied { at }),
        ]
    );
}

#[test]
fn two_timed_requests_split_a_block_into_three_segments() {
    let mut rig = level_rig();
    let start = rig.graph.current_sample;
    submit(&rig, "level", 0, 0.5, When::AtSample(start + 40));
    submit(&rig, "level", 0, 0.25, When::AtSample(start + 10));

    let out = rig.render(1);
    assert!(out[..10].iter().all(|v| *v == 0.0), "{out:?}");
    assert!(out[10..40].iter().all(|v| *v == 0.25), "{out:?}");
    assert!(out[40..].iter().all(|v| *v == 0.5), "{out:?}");
    assert_eq!(rig.graph.current_sample, start + 64);
}

#[test]
fn overdue_requests_keep_their_time_order() {
    let mut rig = level_rig();
    let start = rig.graph.current_sample;
    // Received in reverse time order: the later time is the last value.
    let later = submit(&rig, "level", 0, 0.25, When::AtSample(start - 10));
    let earlier = submit(&rig, "level", 0, 0.5, When::AtSample(start - 20));
    assert!(rig.render(1).iter().all(|v| *v == 0.25));
    let late = |due| Outcome::AppliedLate { at: start, due };
    assert_eq!(
        outcomes(&mut rig),
        [(earlier, late(start - 20)), (later, late(start - 10))]
    );
}

#[test]
fn without_a_module_that_accepts_it_a_request_is_refused() {
    let mut rig = level_rig();
    rig.graph.request_hook = None;
    let id = submit(&rig, "level", 0, 0.5, When::Now);
    assert!(rig.render(1).iter().all(|v| *v == 0.0));
    assert_eq!(
        outcomes(&mut rig),
        [(id, Outcome::Refused(Refusal::Unsupported))]
    );
}

#[test]
fn a_block_with_requests_but_none_due_renders_as_before() {
    let mut requested = level_rig();
    let mut untouched = level_rig();
    let far = requested.graph.current_sample + 64 * 100;
    submit(&requested, "level", 0, 0.5, When::AtSample(far));
    assert_eq!(requested.render(4), untouched.render(4));
}

/// One block, counting its allocator events.
pub(super) fn counted_block(rig: &mut Rig) -> (usize, usize) {
    let mut left = [0.0f32; 64];
    let mut right = [0.0f32; 64];
    let ((), allocs, frees) = allocator_events(|| rig.graph.process_block(&mut left, &mut right));
    (allocs, frees)
}

/// The base rig, hooked, with requests to an oscillator's control
/// `frequency` landing on its `frequency` input.
pub(super) fn oscillator_rig() -> (Rig, u16) {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    hook(&mut rig);
    let port = rig.graph.modules["osc1"]
        .module()
        .input_port_index("frequency")
        .unwrap();
    (rig, port as u16)
}

pub(super) fn frequency(rig: &mut Rig, id: &str, port: u16) -> f32 {
    let module = rig.graph.modules.get_mut(id).unwrap().module_mut();
    module.input_block_mut(usize::from(port))[0]
}

#[test]
fn coalescing_and_applying_mid_block_are_clean() {
    let (mut rig, port) = oscillator_rig();
    let start = rig.graph.current_sample;
    let first = submit(&rig, "osc1", port, 0.1, When::AtSample(start + 10));
    let last = submit(&rig, "osc1", port, 0.2, When::AtSample(start + 10));
    let other = submit(&rig, "osc2", port, 0.3, When::AtSample(start + 30));

    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(frequency(&mut rig, "osc1", port), 0.2);
    assert_eq!(frequency(&mut rig, "osc2", port), 0.3);
    assert_eq!(
        outcomes(&mut rig),
        [
            (first, Outcome::Superseded),
            (last, Outcome::Applied { at: start + 10 }),
            (other, Outcome::Applied { at: start + 30 }),
        ]
    );
}

#[test]
fn overdue_requests_for_different_targets_apply_in_time_order() {
    let (mut rig, port) = oscillator_rig();
    let start = rig.graph.current_sample;
    let later = submit(&rig, "osc1", port, 0.25, When::AtSample(start - 5));
    let earlier = submit(&rig, "osc2", port, 0.5, When::AtSample(start - 40));
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(frequency(&mut rig, "osc1", port), 0.25);
    assert_eq!(frequency(&mut rig, "osc2", port), 0.5);
    let late = |due| Outcome::AppliedLate { at: start, due };
    assert_eq!(
        outcomes(&mut rig),
        [(earlier, late(start - 40)), (later, late(start - 5))]
    );
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "request_channel called on the audio thread")]
fn process_block_refuses_control_only_calls_in_debug() {
    let mut rig = Rig::new(BASE);
    rig.render(1);
    rig.graph.request_hook = Some(|_, _, _, _, _| {
        drop(crate::control_request::request_channel(
            4,
            Default::default(),
        ));
        Ok(())
    });
    submit(&rig, "osc1", 0, 0.5, When::Now);
    rig.render(1);
}
