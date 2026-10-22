//! Where an offline render's `ended` gates rose within its block.
//!
//! A render block can process in pieces: the graph splits it at each
//! request due inside it (a scheduler's table, a ramp), a request timed by
//! a condition splits a module's span, and a replay splits the render's
//! own block (`RenderEngine::replay`). A module's output buffer then holds
//! only the last piece, so the watch scans the graph's copy of each
//! `ended` output after every segment, keeping the frame, counted from the
//! render block's start, where it first went high.
//!
//! Only an offline render installs one: a live graph never looks.

use indexmap::IndexMap;

use crate::invention::runtime::ModuleInstance;

/// Whether an output port is a one-shot module's latched end-of-playback
/// gate: `ended` by the convention. Not `end`, which the sample modules name
/// a one-sample pulse at the end of a sample or slice.
pub(crate) fn is_end_output(port: &str) -> bool {
    port == "ended"
}

/// The `ended` gates of a graph's modules, and where each rose this block.
#[derive(Debug, Default)]
pub(crate) struct EndWatch {
    /// Each watched module's id, index and `ended` port, as compiled.
    gates: Vec<(String, usize, usize)>,
    /// Parallel to `gates`: the frame of the block the gate first went high.
    rises: Vec<Option<usize>>,
    /// Frames processed since the block began.
    at: usize,
}

impl EndWatch {
    /// Finds the gates of `modules` again, as the topology recompiles: the
    /// modules may have changed. Allocates.
    pub(crate) fn refresh(&mut self, modules: &IndexMap<String, ModuleInstance>) {
        let gates = modules
            .iter()
            .enumerate()
            .filter_map(|(index, (id, module))| {
                let outputs = module.module().outputs();
                let port = outputs.iter().position(|port| is_end_output(port))?;
                Some((id.clone(), index, port))
            });
        let kept: Vec<_> = gates.collect();
        if kept != self.gates {
            self.rises = vec![None; kept.len()];
            self.gates = kept;
        }
    }

    /// Begins a render block.
    pub(crate) fn begin(&mut self) {
        self.rises.fill(None);
        self.at = 0;
    }

    /// Records the segment of `frames` frames just processed, from the
    /// graph's output copies `out_bufs` (port stride `capacity`).
    /// Allocation-free.
    pub(crate) fn record(&mut self, out_bufs: &[Vec<f32>], capacity: usize, frames: usize) {
        for (&(_, module, port), rise) in self.gates.iter().zip(&mut self.rises) {
            if rise.is_some() {
                continue;
            }
            let Some(out) = out_bufs.get(module) else {
                continue;
            };
            let start = port * capacity;
            let Some(segment) = out.get(start..start + frames) else {
                continue;
            };
            *rise = segment
                .iter()
                .position(|&value| value > 0.5)
                .map(|k| self.at + k);
        }
        self.at += frames;
    }

    /// Where the `ended` gate of module `id` first went high this block
    /// (`Some(None)`: it did not), or `None` when the block did not render
    /// it. By id: an edit since may have moved the modules.
    pub(crate) fn rise(&self, id: &str) -> Option<Option<usize>> {
        let at = self.gates.iter().position(|(module, ..)| module == id)?;
        Some(self.rises[at])
    }
}
