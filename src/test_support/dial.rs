//! A test module with declared controls, built by [`DialFactory`] as type
//! `dial`: it outputs its `level` (clamped to 0..=1 as it applies) plus one
//! per `pulse` event, and holds a `shape` choice it refuses past its
//! options, and integers: `steps` and `span` clamp, `index` does not.

use std::sync::Arc;

use crate::control_request::{
    ControlCells, ControlDecl, ControlIndex, ControlTable, DeclKind, Refusal, RtValue,
};
use crate::invention::declared::DeclaredSurface;
use crate::{GraphModule, Module, ModuleBuildResult, ModuleFactory, MAX_BLOCK};

pub(crate) const DIAL: &str = "dial";
pub(crate) const LEVEL: ControlIndex = ControlIndex(0);
pub(crate) const SHAPE: ControlIndex = ControlIndex(1);
pub(crate) const PULSE: ControlIndex = ControlIndex(2);
pub(crate) const SLOPE: ControlIndex = ControlIndex(3);
pub(crate) const HELD: ControlIndex = ControlIndex(4);
pub(crate) const STEPS: ControlIndex = ControlIndex(5);
pub(crate) const SPAN: ControlIndex = ControlIndex(6);
pub(crate) const INDEX: ControlIndex = ControlIndex(7);

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "level",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.25),
        "Output level",
    )
    .clamped(0.0, 1.0),
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
    ControlDecl::new(
        "slope",
        DeclKind::Choice(&["steep", "flat", "gentle"]),
        RtValue::U32(0),
        "The shape options, ordered otherwise",
    ),
    ControlDecl::new("held", DeclKind::Bool, RtValue::Bool(false), "A plain flag"),
    ControlDecl::new(
        "steps",
        DeclKind::Integer { min: 1, max: 8 },
        RtValue::I32(4),
        "Clamps any whole number",
    )
    .clamped(1.0, 8.0),
    ControlDecl::new(
        "span",
        DeclKind::Integer { min: 1, max: 64 },
        RtValue::I32(4),
        "Clamps any whole number, to a wider range",
    )
    .clamped(1.0, 64.0),
    ControlDecl::new(
        "index",
        DeclKind::Integer { min: 0, max: 100 },
        RtValue::I32(0),
        "Takes only its range",
    ),
];

static TABLE: ControlTable = ControlTable::of(DECLS);

/// Outputs its `level`, clamped to 0..=1 as it applies, plus one per pulse.
pub(crate) struct Dial {
    level: f32,
    shape: u32,
    slope: u32,
    held: bool,
    pulses: u32,
    cells: Arc<ControlCells>,
    out: [f32; MAX_BLOCK],
}

pub(crate) struct DialFactory;

impl ModuleFactory for DialFactory {
    fn type_id(&self) -> &'static str {
        DIAL
    }

    fn build(
        &self,
        _sample_rate: u32,
        _config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let cells = Arc::new(ControlCells::new(DECLS.iter().map(|decl| decl.default)));
        let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
        let dial = Dial {
            level: 0.25,
            shape: 0,
            slope: 0,
            held: false,
            pulses: 0,
            cells,
            out: [0.0; MAX_BLOCK],
        };
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(dial)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
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
            (SLOPE, RtValue::U32(slope)) if slope < 3 => {
                self.slope = slope;
                Ok(value)
            }
            (HELD, RtValue::Bool(held)) => {
                self.held = held;
                Ok(value)
            }
            (STEPS | SPAN | INDEX, RtValue::I32(whole)) => {
                let (decl, _) = TABLE.decl(control).ok_or(Refusal::Unsupported)?;
                let DeclKind::Integer { min, max } = decl.kind else {
                    return Err(Refusal::Unsupported);
                };
                // Held in its cell alone: nothing it outputs reads it.
                Ok(RtValue::I32(whole.clamp(min, max)))
            }
            (PULSE, RtValue::Bool(true)) => {
                self.pulses += 1;
                Ok(value)
            }
            _ => Err(Refusal::Unsupported),
        }
    }
}
