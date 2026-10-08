//! A module with declared controls on a live graph: requests reach its
//! `apply` through the drain, on the audio thread, without allocating, and
//! what each control then holds is published to its cells.

use super::requests::{counted_block, outcomes};
use super::*;
use crate::control_request::{
    ControlCells, ControlDecl, ControlIndex, ControlTable, DeclKind, Outcome, Refusal, Request,
    RequestId, RequestValue, RtValue, When,
};
use crate::{GraphModule, Module, ModuleBuildResult, ModuleFactory, MAX_BLOCK};

const DIAL: &str = "dial";
const LEVEL: ControlIndex = ControlIndex(0);
const SHAPE: ControlIndex = ControlIndex(1);
const PULSE: ControlIndex = ControlIndex(2);

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "level",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.25),
        "Output level",
    ),
    ControlDecl::new(
        "shape",
        DeclKind::Choice(&["flat", "steep"]),
        RtValue::U32(0),
        "Shape",
    ),
    ControlDecl::new(
        "pulse",
        DeclKind::Bool,
        RtValue::Bool(false),
        "Counts a pulse",
    )
    .event(),
];

static TABLE: ControlTable = ControlTable::of(DECLS);

/// Outputs its `level`, clamped to 0..=1 as it applies, plus one per pulse.
pub(super) struct Dial {
    level: f32,
    shape: u32,
    pulses: u32,
    cells: ControlCells,
    out: [f32; MAX_BLOCK],
}

struct DialFactory;

impl ModuleFactory for DialFactory {
    fn type_id(&self) -> &'static str {
        DIAL
    }

    fn build(
        &self,
        _sample_rate: u32,
        _config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let cells = ControlCells::new(DECLS.iter().map(|decl| decl.default));
        let dial = Dial {
            level: 0.25,
            shape: 0,
            pulses: 0,
            cells,
            out: [0.0; MAX_BLOCK],
        };
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(dial)),
            handles: Vec::new(),
            control_surface: None,
            sink: None,
        })
    }
}

impl Module for Dial {
    fn name(&self) -> &str {
        "Dial"
    }

    fn process(&mut self, frames: usize) -> bool {
        self.out[..frames].fill(self.level + self.pulses as f32);
        true
    }

    fn inputs(&self) -> &[&str] {
        &[]
    }

    fn outputs(&self) -> &[&str] {
        &["out"]
    }

    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        &mut self.out
    }

    fn output_block(&self, _index: usize) -> &[f32] {
        &self.out
    }

    fn set_input(&mut self, port: &str, _value: f32) -> Result<(), String> {
        Err(format!("no input '{port}'"))
    }

    fn get_output(&self, _port: &str) -> Result<f32, String> {
        Ok(self.out[0])
    }

    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        match (control, value) {
            (LEVEL, RtValue::F32(level)) => {
                self.level = level.clamp(0.0, 1.0);
                Ok(RtValue::F32(self.level))
            }
            (SHAPE, RtValue::U32(shape)) if shape < 2 => {
                self.shape = shape;
                Ok(value)
            }
            (SHAPE, RtValue::U32(_)) => Err(Refusal::Invalid),
            (PULSE, RtValue::Bool(true)) => {
                self.pulses += 1;
                Ok(value)
            }
            _ => Err(Refusal::Unsupported),
        }
    }
}

/// A dial alone into an unclipped dac, so the output is what it holds.
fn dial_rig() -> Rig {
    let mut rig = Rig::new(
        r#"{
            "version": "1.0.0",
            "modules": [{ "id": "dac", "type": "dac", "config": { "soft_clip": false } }],
            "connections": []
        }"#,
    );
    rig.registry.register(DialFactory);
    let dial = rig.build("dial", DIAL, serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("dial", dial);
            change.connect(edge("dial", "out", "dac", "audio"))
        })
        .unwrap();
    rig.render(1);
    rig
}

/// Submits `value` for `control` of the dial as a front door would.
fn submit(rig: &Rig, control: ControlIndex, value: RtValue, at: u64, event: bool) -> RequestId {
    let mut publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("dial", control).unwrap();
    let mut request = Request::new(target, RequestValue::Value(value));
    request.when = When::AtSample(at);
    request.event = event;
    let id = rig.live.requests.submit(request).unwrap();
    publisher.note_written();
    id
}

fn held(rig: &Rig, control: ControlIndex) -> RtValue {
    let module = rig.graph.modules["dial"].module();
    module.declared().unwrap().1.load(control).unwrap()
}

#[test]
fn a_request_reaches_apply_at_its_sample_and_is_read_back() {
    let mut rig = dial_rig();
    let start = rig.graph.current_sample;
    let id = submit(&rig, LEVEL, RtValue::F32(2.0), start + 10, false);
    assert_eq!(held(&rig, LEVEL), RtValue::F32(0.25));

    let mut left = [0.0f32; 64];
    let mut right = [0.0f32; 64];
    let ((), allocs, frees) =
        crate::alloc_counter::allocator_events(|| rig.graph.process_block(&mut left, &mut right));
    assert_eq!((allocs, frees), (0, 0));
    assert!(left[..10].iter().all(|v| *v == 0.25), "{left:?}");
    assert!(left[10..].iter().all(|v| *v == 1.0), "{left:?}");
    assert_eq!(held(&rig, LEVEL), RtValue::F32(1.0), "the clamped value");
    let applied = Outcome::Applied { at: start + 10 };
    assert_eq!(outcomes(&mut rig), [(id, applied)]);
}

#[test]
fn a_value_the_module_cannot_hold_is_refused_and_changes_nothing() {
    let mut rig = dial_rig();
    let start = rig.graph.current_sample;
    let id = submit(&rig, SHAPE, RtValue::U32(2), start, false);
    assert_eq!(counted_block(&mut rig), (0, 0));
    assert_eq!(held(&rig, SHAPE), RtValue::U32(0));
    assert_eq!(
        outcomes(&mut rig),
        [(id, Outcome::Refused(Refusal::Invalid))]
    );
}

#[test]
fn two_events_at_one_sample_both_fire_where_values_coalesce() {
    let mut rig = dial_rig();
    let at = rig.graph.current_sample + 5;
    let pulses = [true, true].map(|event| submit(&rig, PULSE, RtValue::Bool(true), at, event));
    let first = submit(&rig, LEVEL, RtValue::F32(0.5), at, false);
    let last = submit(&rig, LEVEL, RtValue::F32(0.75), at, false);
    assert_eq!(counted_block(&mut rig), (0, 0));
    let out = rig.render(1);
    assert!(out.iter().all(|v| *v == 2.75), "{out:?}");
    let applied = Outcome::Applied { at };
    assert_eq!(
        outcomes(&mut rig),
        [
            (first, Outcome::Superseded),
            (pulses[0], applied),
            (pulses[1], applied),
            (last, applied),
        ]
    );
}
