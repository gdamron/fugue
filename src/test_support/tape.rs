//! A test module with a payload control, built by [`TapeFactory`] as type
//! `tape`: it keeps `notes`, a JSON array of numbers, whole as a payload
//! and outputs their sum times its `gain`. Its notes count their drops on
//! the thread that frees them ([`freed`]).

use std::cell::Cell;
use std::sync::Arc;

use crate::control_request::{
    ControlCells, ControlDecl, ControlIndex, ControlTable, DeclKind, PayloadCodec, Refusal, RtValue,
};
use crate::invention::declared::DeclaredSurface;
use crate::payload::{Payload, Retired, Shared};
use crate::{ControlValue, GraphModule, Module, ModuleBuildResult, ModuleFactory, MAX_BLOCK};

pub(crate) const TAPE: &str = "tape";
pub(crate) const GAIN: ControlIndex = ControlIndex(0);
pub(crate) const NOTES: ControlIndex = ControlIndex(1);

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "gain",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(1.0),
        "Gain",
    )
    .clamped(0.0, 1.0),
    ControlDecl::payload("notes", CODEC, "Notes as a JSON array"),
];

const CODEC: PayloadCodec = PayloadCodec { prepare };

static TABLE: ControlTable = ControlTable::of(DECLS);

thread_local! {
    static FREED: Cell<usize> = const { Cell::new(0) };
}

/// How many notes payloads this thread has freed.
pub(crate) fn freed() -> usize {
    FREED.with(Cell::get)
}

/// A tape's notes; counts its drop on the thread freeing it.
pub(crate) struct Notes(pub(crate) Vec<f32>);

impl Drop for Notes {
    fn drop(&mut self) {
        FREED.with(|freed| freed.set(freed.get() + 1));
    }
}

fn parse(text: &str) -> Result<(Notes, ControlValue), String> {
    let notes: Vec<f32> = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let shown = serde_json::to_string(&notes).map_err(|error| error.to_string())?;
    Ok((Notes(notes), shown.into()))
}

fn prepare(value: &ControlValue) -> Result<(Payload, ControlValue), String> {
    let (notes, shown) = parse(value.as_string()?)?;
    Ok((Payload::new(notes), shown))
}

pub(crate) struct Tape {
    gain: f32,
    notes: Shared<Notes>,
    cells: Arc<ControlCells>,
    out: [f32; MAX_BLOCK],
}

pub(crate) struct TapeFactory;

impl ModuleFactory for TapeFactory {
    fn type_id(&self) -> &'static str {
        TAPE
    }

    fn build(
        &self,
        _sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        // An array, or the JSON text of one that a recorded write leaves.
        let text = match config.get("notes") {
            Some(serde_json::Value::String(text)) => text.clone(),
            Some(value) => value.to_string(),
            None => "[]".to_string(),
        };
        let (notes, shown) = parse(&text)?;
        let cells = Arc::new(ControlCells::new(DECLS.iter().map(|decl| decl.default)));
        let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
        surface.show_payload("notes", shown);
        let tape = Tape {
            gain: 1.0,
            notes: Shared::new(notes),
            cells,
            out: [0.0; MAX_BLOCK],
        };
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(tape)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

impl Module for Tape {
    fn name(&self) -> &str {
        "Tape"
    }

    fn process(&mut self, frames: usize) -> bool {
        let sum: f32 = self.notes.0.iter().sum();
        self.out[..frames].fill(self.gain * sum);
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
            (GAIN, RtValue::F32(gain)) => {
                self.gain = gain.clamp(0.0, 1.0);
                Ok(RtValue::F32(self.gain))
            }
            _ => Err(Refusal::Unsupported),
        }
    }

    fn apply_payload(
        &mut self,
        control: ControlIndex,
        payload: Payload,
    ) -> Result<Retired, (Refusal, Payload)> {
        if control != NOTES {
            return Err((Refusal::Unsupported, payload));
        }
        match payload.downcast::<Notes>() {
            Ok(notes) => Ok(std::mem::replace(&mut self.notes, notes).into()),
            Err(payload) => Err((Refusal::Invalid, payload)),
        }
    }
}
