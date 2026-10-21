//! A test module playing a [`TestTable`], built by [`TablerFactory`] as
//! type `tabler`: its table is the one [`next_table`] last set on the
//! building thread (empty when none), gated on its `clock` input. It
//! outputs 1 while a drain plays its table, else 0.

use std::cell::RefCell;
use std::sync::atomic::Ordering;

use super::table::TestTable;
use crate::control_request::BeatTable;
use crate::{GraphModule, Module, ModuleBuildResult, ModuleFactory, MAX_BLOCK};

pub(crate) const TABLER: &str = "tabler";

thread_local! {
    static NEXT: RefCell<Option<TestTable>> = const { RefCell::new(None) };
}

/// The table the next `tabler` built on this thread plays.
pub(crate) fn next_table(table: TestTable) {
    NEXT.with(|next| *next.borrow_mut() = Some(table));
}

pub(crate) struct Tabler {
    table: TestTable,
    clock: [f32; MAX_BLOCK],
    out: [f32; MAX_BLOCK],
}

pub(crate) struct TablerFactory;

impl ModuleFactory for TablerFactory {
    fn type_id(&self) -> &'static str {
        TABLER
    }

    fn build(
        &self,
        _sample_rate: u32,
        _config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let table = NEXT.with(|next| next.borrow_mut().take());
        let tabler = Tabler {
            table: table.unwrap_or_else(|| TestTable::new(0, Vec::new())),
            clock: [0.0; MAX_BLOCK],
            out: [0.0; MAX_BLOCK],
        };
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(tabler)),
            handles: Vec::new(),
            control_surface: None,
            sink: None,
        })
    }
}

impl Module for Tabler {
    fn name(&self) -> &str {
        "Tabler"
    }

    fn process(&mut self, frames: usize) -> bool {
        let played = self.table.played.load(Ordering::Relaxed);
        self.out[..frames].fill(f32::from(u8::from(played)));
        true
    }

    fn inputs(&self) -> &[&str] {
        &["clock"]
    }

    fn outputs(&self) -> &[&str] {
        &["played"]
    }

    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        &mut self.clock
    }

    fn output_block(&self, _index: usize) -> &[f32] {
        &self.out
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "clock" => {
                self.clock.fill(value);
                Ok(())
            }
            _ => Err(format!("no input '{port}'")),
        }
    }

    fn get_output(&self, _port: &str) -> Result<f32, String> {
        Ok(self.out[0])
    }

    #[allow(private_interfaces)]
    fn beat_table(&self) -> Option<&dyn BeatTable> {
        Some(&self.table)
    }
}
