//! Where a split block's `ended` gates rose.
//!
//! A replayed request due inside a block splits it, and the graph then
//! processes the block in pieces: a module's output buffer holds only the
//! last piece afterwards. So that [`super::RenderEngine::scan_end_gate`]
//! still answers for the whole block, each piece of a split block is
//! scanned as it is processed, keeping per module the block frame its
//! `ended` gate first went high. A block processed whole is not scanned
//! here: its buffers already hold all of it.

use crate::invention::graph::SignalGraph;
use crate::Module;

/// The output port index of `module`'s `ended` gate, if it has one.
pub(super) fn end_port(module: &dyn Module) -> Option<usize> {
    module
        .outputs()
        .iter()
        .position(|port| super::is_end_output(port))
}

/// The `ended` rises of the last block rendered, when it was split.
#[derive(Default)]
pub(super) struct SplitEnds {
    /// Whether the last block rendered was split: `rises` describes it only
    /// then.
    split: bool,
    /// By the graph's module position, the frame within the block where the
    /// module's `ended` gate first went high.
    rises: Vec<Option<usize>>,
}

impl SplitEnds {
    /// Makes room for `modules` modules, so recording a split block does
    /// not allocate (set up with the replay that splits blocks).
    pub(super) fn reserve(&mut self, modules: usize) {
        if self.rises.len() < modules {
            self.rises.resize(modules, None);
        }
    }

    /// Starts a block: until a piece of it is recorded, it is whole.
    pub(super) fn start_block(&mut self) {
        self.split = false;
    }

    /// Records the piece of a split block just processed: `frames` frames
    /// from the block's frame `at`.
    pub(super) fn record_piece(&mut self, graph: &SignalGraph, at: usize, frames: usize) {
        let modules = graph.modules.len();
        if !self.split {
            self.split = true;
            // Allocates only when the graph grew since the replay began.
            self.reserve(modules);
            self.rises[..modules].fill(None);
        }
        for (rise, module) in self.rises.iter_mut().zip(graph.modules.values()) {
            if rise.is_some() {
                continue;
            }
            let module = module.module();
            let Some(port) = end_port(module) else {
                continue;
            };
            let block = module.output_block(port);
            let piece = &block[..frames.min(block.len())];
            *rise = piece.iter().position(|&value| value > 0.5).map(|k| at + k);
        }
    }

    /// Where the `ended` gate of the module at graph position `index` first
    /// went high in the last block, when that block was split (`None`
    /// inside when it did not); `None` when the block was processed whole.
    pub(super) fn rise(&self, index: usize) -> Option<Option<usize>> {
        self.split.then(|| self.rises.get(index).copied().flatten())
    }
}
