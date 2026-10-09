//! A test module with declared controls, built by [`DialFactory`] as type
//! `dial`: it outputs its `level` (clamped to 0..=1 as it applies) plus one
//! per `pulse` event, and holds a `shape` choice it refuses past its
//! options.

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
pub(crate) struct Dial {
    level: f32,
    shape: u32,
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
            (PULSE, RtValue::Bool(true)) => {
                self.pulses += 1;
                Ok(value)
            }
            _ => Err(Refusal::Unsupported),
        }
    }
}
