//! Audio-thread processing of a development's inner graph, scheduled as the
//! main graph schedules its own (see `crate::invention::graph::process`):
//! the routes into a port sum, each acyclic module runs a whole block at a
//! time, a feedback group through a declared delay runs in chunks of it,
//! and any other feedback group runs sample by sample, each back-edge
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
            let chunk = match graph.written_within[group] {
                true => None,
                false => graph.groups[group].chunks.as_ref().map(|c| c.frames),
            };
            if chunk == Some(1) {
                run_unit_chunks(graph, inputs, group, frames);
            } else if chunk.is_some() {
                run_chunked_group(graph, inputs, group, frames);
            } else if graph.groups[group].feedback {
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
    run_span(graph, inputs, m, 0, frames, Pass::Process, true);
}

/// What [`run_span`] has a module do over its span.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    Process,
    Emit,
    Absorb,
}

/// Runs module `m` over frames `from..from + frames` of the block as a
/// block of its own: its routes summed into its inputs (unless it emits
/// and `gathers` is false), then `pass`, then (unless it absorbs) its
/// outputs stored at `from`.
fn run_span(
    graph: &mut CompiledDevelopmentGraph,
    inputs: &[[f32; MAX_BLOCK]],
    m: usize,
    from: usize,
    frames: usize,
    pass: Pass,
    gathers: bool,
) {
    let module = graph.modules[m].module_mut();
    // A cut that reads nothing as it emits has its inputs gathered only to
    // absorb them.
    let routes = if pass == Pass::Emit && !gathers {
        0
    } else {
        graph.routes[m].len()
    };
    for r in 0..routes {
        let route: InnerRoute = graph.routes[m][r];
        let src = match route.source {
            Source::Module { module, port } => {
                let base = port * MAX_BLOCK + from;
                &graph.out_bufs[module][base..base + frames]
            }
            Source::Input(input) => &inputs[input][from..from + frames],
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

    if pass == Pass::Absorb {
        module.absorb(frames);
        return;
    }
    // A span after the first starts at its own frame 0.
    let _frame_zero = (from > 0).then(|| FrameScope::frame(from));
    take_automation(module);
    if pass == Pass::Emit {
        module.emit(frames);
    } else {
        module.process(frames);
    }

    for p in 0..graph.out_counts[m] {
        let base = p * MAX_BLOCK + from;
        let src = module.output_block(p);
        graph.out_bufs[m][base..base + frames].copy_from_slice(&src[..frames]);
    }
}

/// Runs feedback group `group` in chunks of its shortest declared delay,
/// as the main graph runs its own (see `crate::invention::graph::chunks`):
/// the cuts emit, the rest run over the chunk in order, the cuts absorb.
fn run_chunked_group(
    graph: &mut CompiledDevelopmentGraph,
    inputs: &[[f32; MAX_BLOCK]],
    group: usize,
    frames: usize,
) {
    let Some(chunk) = graph.groups[group].chunks.as_ref().map(|c| c.frames) else {
        return;
    };
    let member = |graph: &CompiledDevelopmentGraph, k: usize, cut: bool| {
        let chunks = graph.groups[group].chunks.as_ref()?;
        let m = if cut {
            chunks.cuts.get(k)
        } else {
            chunks.rest.get(k)
        };
        Some((
            *m?,
            !cut || chunks.emit_reads.get(k).copied().unwrap_or(true),
        ))
    };
    let mut from = 0;
    while from < frames {
        let n = chunk.min(frames - from);
        for pass in [Pass::Emit, Pass::Process, Pass::Absorb] {
            let cut = pass != Pass::Process;
            let mut k = 0;
            while let Some((m, gathers)) = member(graph, k, cut) {
                run_span(graph, inputs, m, from, n, pass, gathers);
                k += 1;
            }
        }
        from += n;
    }
}

/// Runs feedback group `group` sample by sample, each member in its
/// per-sample order, so a back-edge observes a one-sample delay whatever
/// the block size.
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
            run_frame(graph, inputs, m, s, Pass::Process, true, true);
        }
    }
}

/// Runs a chunked group whose chunks are a single frame a frame at a time,
/// as [`run_feedback_group`] does, in the chunks' order.
fn run_unit_chunks(
    graph: &mut CompiledDevelopmentGraph,
    inputs: &[[f32; MAX_BLOCK]],
    group: usize,
    frames: usize,
) {
    let (cuts, rest) = graph.groups[group]
        .chunks
        .as_ref()
        .map_or((0, 0), |c| (c.cuts.len(), c.rest.len()));
    let member = |graph: &CompiledDevelopmentGraph, k: usize, cut: bool| {
        let chunks = graph.groups[group].chunks.as_ref()?;
        let m = if cut {
            chunks.cuts.get(k)
        } else {
            chunks.rest.get(k)
        };
        Some((
            *m?,
            !cut || chunks.emit_reads.get(k).copied().unwrap_or(true),
        ))
    };
    for s in 0..frames {
        let _frame_zero = FrameScope::frame(s);
        for (pass, count) in [
            (Pass::Emit, cuts),
            (Pass::Process, rest),
            (Pass::Absorb, cuts),
        ] {
            for k in 0..count {
                if let Some((m, gathers)) = member(graph, k, pass != Pass::Process) {
                    run_frame(graph, inputs, m, s, pass, false, gathers);
                }
            }
        }
    }
}

/// Runs module `m` for frame `s` of a group run a frame at a time: its
/// routes summed into its inputs' frame 0 (unless it emits and `gathers`
/// is false; a back-edge reading the frame before when `delays`), then
/// `pass`, then (unless it absorbs) its outputs stored.
// The frame indexes the inputs, every member's outputs and the carry alike.
#[allow(clippy::needless_range_loop)]
fn run_frame(
    graph: &mut CompiledDevelopmentGraph,
    inputs: &[[f32; MAX_BLOCK]],
    m: usize,
    s: usize,
    pass: Pass,
    delays: bool,
    gathers: bool,
) {
    let module = graph.modules[m].module_mut();
    let routes = if pass == Pass::Emit && !gathers {
        0
    } else {
        graph.routes[m].len()
    };
    for r in 0..routes {
        let route = graph.routes[m][r];
        let value = match route.source {
            Source::Input(input) => inputs[input][s],
            Source::Module { module, port } if route.delayed && delays => {
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

    if pass == Pass::Absorb {
        module.absorb(1);
        return;
    }
    take_automation(module);
    if pass == Pass::Emit {
        module.emit(1);
    } else {
        module.process(1);
    }

    for p in 0..graph.out_counts[m] {
        graph.out_bufs[m][p * MAX_BLOCK + s] = module.output_block(p)[0];
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
