//! Audio-thread processing of a development's inner graph, scheduled as the
//! main graph schedules its own (see `crate::invention::graph::process`):
//! the routes into a port sum, each acyclic module runs a whole block at a
//! time, and only a feedback group runs sample by sample, each back-edge
//! reading its source's previous sample. Allocation-, lock- and free-free.

use super::compiled_graph::{CompiledDevelopmentGraph, InnerRoute, Source};
use super::DevelopmentModule;
use crate::control_request::{take_automation, FrameScope};
use crate::MAX_BLOCK;

impl DevelopmentModule {
    /// Processes the inner graph for `frames` frames (at most `MAX_BLOCK`),
    /// from the development's input buffers to its output buffers.
    pub(super) fn process_graph(&mut self, frames: usize) {
        let inputs = &self.input_buffers;
        let graph = &mut self.graph;
        graph.current_sample += frames as u64;

        for gi in 0..graph.group_order.len() {
            let group = graph.group_order[gi];
            if graph.groups[group].feedback {
                run_feedback_group(graph, inputs, group, frames);
            } else {
                let module = graph.groups[group].members[0];
                run_block(graph, inputs, module, frames);
            }
        }

        if frames > 0 {
            store_carry(graph, frames);
        }

        // Cache external output ports.
        for oi in 0..self.outputs.len() {
            let from = self.outputs[oi].module_index;
            let base = self.outputs[oi].port_index * MAX_BLOCK;
            self.output_buffers[oi][..frames]
                .copy_from_slice(&self.graph.out_bufs[from][base..base + frames]);
        }
    }
}

/// Runs acyclic module `m` over the whole block: its routes summed into
/// its inputs, then one `process(frames)`.
fn run_block(
    graph: &mut CompiledDevelopmentGraph,
    inputs: &[[f32; MAX_BLOCK]],
    m: usize,
    frames: usize,
) {
    let module = graph.modules[m].module_mut();
    for r in 0..graph.routes[m].len() {
        let route: InnerRoute = graph.routes[m][r];
        let src = match route.source {
            Source::Module { module, port } => {
                let base = port * MAX_BLOCK;
                &graph.out_bufs[module][base..base + frames]
            }
            Source::Input(input) => &inputs[input][..frames],
        };
        let dst = &mut module.input_block_mut(route.to_port)[..frames];
        if route.first {
            for k in 0..frames {
                dst[k] = 0.0 + src[k];
            }
        } else {
            for k in 0..frames {
                dst[k] += src[k];
            }
        }
    }

    take_automation(module);
    module.process(frames);

    for p in 0..graph.out_counts[m] {
        let base = p * MAX_BLOCK;
        let src = module.output_block(p);
        graph.out_bufs[m][base..base + frames].copy_from_slice(&src[..frames]);
    }
}

/// Runs feedback group `group` sample by sample, each member in its
/// per-sample order, so a back-edge observes a one-sample delay whatever
/// the block size.
// The frame indexes the inputs, every member's outputs and the carry alike.
#[allow(clippy::needless_range_loop)]
fn run_feedback_group(
    graph: &mut CompiledDevelopmentGraph,
    inputs: &[[f32; MAX_BLOCK]],
    group: usize,
    frames: usize,
) {
    for s in 0..frames {
        // Where an inner scheduler's entries fall (see
        // `control_request::hold`).
        let _frame_zero = FrameScope::frame(s);
        for mi in 0..graph.groups[group].members.len() {
            let m = graph.groups[group].members[mi];
            let module = graph.modules[m].module_mut();
            for r in 0..graph.routes[m].len() {
                let route = graph.routes[m][r];
                let value = match route.source {
                    Source::Input(input) => inputs[input][s],
                    Source::Module { module, port } if route.delayed => {
                        if s == 0 {
                            graph.out_prev[module][port]
                        } else {
                            graph.out_bufs[module][port * MAX_BLOCK + (s - 1)]
                        }
                    }
                    Source::Module { module, port } => graph.out_bufs[module][port * MAX_BLOCK + s],
                };
                let dst = &mut module.input_block_mut(route.to_port)[0];
                if route.first {
                    *dst = 0.0 + value;
                } else {
                    *dst += value;
                }
            }

            take_automation(module);
            module.process(1);

            for p in 0..graph.out_counts[m] {
                graph.out_bufs[m][p * MAX_BLOCK + s] = module.output_block(p)[0];
            }
        }
    }
}

/// Records each feedback member's last output sample, which a back-edge
/// reads at the next call's frame 0.
fn store_carry(graph: &mut CompiledDevelopmentGraph, frames: usize) {
    for g in 0..graph.groups.len() {
        if !graph.groups[g].feedback {
            continue;
        }
        for mi in 0..graph.groups[g].members.len() {
            let m = graph.groups[g].members[mi];
            for p in 0..graph.out_counts[m] {
                graph.out_prev[m][p] = graph.out_bufs[m][p * MAX_BLOCK + (frames - 1)];
            }
        }
    }
}
