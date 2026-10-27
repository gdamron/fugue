//! Audio-thread block processing for the [`SignalGraph`].

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

use super::conditions::Segment;
use super::SignalGraph;
use crate::audio_thread::AudioThreadScope;
use crate::control_request::{take_automation, FrameScope};

impl SignalGraph {
    /// Processes a block of `frames` frames (`frames == left.len() == right.len()`,
    /// always `<= block_size`), mixing all sink outputs into `left`/`right`.
    ///
    /// Process groups run in topological order: acyclic modules a whole block at
    /// a time, feedback cycles sample-by-sample. Zero heap allocations.
    ///
    /// The block runs as segments split at each pending control request's
    /// sample (see [`super::requests`]); with none due it is one segment.
    /// All of it runs inside an [`AudioThreadScope`], so debug builds catch
    /// a payload dropped, or control-only code reached, from here.
    ///
    /// A zero-length block takes up publications, input writes and requests
    /// as any block start does and applies the requests due at the current
    /// sample, but runs no module and advances no time: how a backend with
    /// no clock settles what was submitted (`NullBackend`). Device streams
    /// and offline renders never ask for one.
    ///
    /// A panic in a module fails the graph on its way out, keeping all it
    /// held for teardown (see [`super::unwind`]): every later block renders
    /// silence and touches nothing.
    pub(crate) fn process_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        let _audio_thread = AudioThreadScope::enter();
        if self.failed {
            left.fill(0.0);
            right.fill(0.0);
            return;
        }
        let rendered = catch_unwind(AssertUnwindSafe(|| self.render_block(left, right)));
        if let Err(panic) = rendered {
            self.fail();
            resume_unwind(panic);
        }
    }

    /// [`Self::process_block`] on a graph that has not failed.
    fn render_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.ensure_process_order();

        let frames = left.len().min(right.len()).min(self.block_capacity);
        if frames == 0 {
            self.apply_due_requests(0);
        }
        let mut start = 0;
        while start < frames {
            let end = start + self.apply_due_requests(frames - start);
            self.process_segment(&mut left[start..end], &mut right[start..end]);
            if let Some(watch) = &mut self.end_watch {
                watch.record(&self.out_bufs, self.block_capacity, end - start);
            }
            start = end;
        }
        self.transport.publish(self.current_sample);
    }

    /// Processes `left.len()` frames (`== right.len()`, at least one) from
    /// the current sample, as one block: every module's input and output
    /// buffers start at frame 0 again, and the feedback carry is stored at
    /// the end, so the next segment continues sample for sample.
    fn process_segment(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len();
        // Where a scheduler's entries fall (see `control_request::hold`).
        let _frame_zero = FrameScope::enter(self.current_sample);
        self.segment = Segment {
            start: self.current_sample,
            frames,
            computed: 0,
        };
        self.current_sample += frames as u64;

        // Requests timed by a condition apply as their target computes
        // (see `super::conditions`), and so do tables played on edges (see
        // `super::edges`); with none waiting, nothing looks.
        let conditioned = self.conditions_waiting() || self.edges_playing();
        for g in 0..self.process_groups.len() {
            if self.process_groups[g].feedback {
                if conditioned {
                    for m in 0..self.process_groups[g].members.len() {
                        self.apply_conditions_on(self.process_groups[g].members[m]);
                    }
                }
                self.process_feedback_group(g, frames);
            } else {
                let module_idx = self.process_groups[g].members[0];
                if !conditioned {
                    self.process_span(module_idx, 0, frames);
                } else if self.sink_indices.contains(&module_idx) {
                    self.apply_conditions_on(module_idx);
                    self.process_span(module_idx, 0, frames);
                } else {
                    self.process_conditioned(module_idx, frames);
                }
            }
            self.segment.computed = g + 1;
        }
        if conditioned {
            self.read_conditions();
            self.commit_edges();
        }

        // Mix sink output blocks.
        for i in 0..frames {
            left[i] = 0.0;
            right[i] = 0.0;
        }
        let sink_count = self.sink_indices.len();
        for si in 0..sink_count {
            let sink_idx = self.sink_indices[si];
            if let Some((_, inst)) = self.modules.get_index(sink_idx) {
                if let Some((l, r)) = inst.sink_block() {
                    for i in 0..frames {
                        left[i] += l[i];
                        right[i] += r[i];
                    }
                }
            }
        }
        if sink_count > 1 {
            let gain = 1.0 / (sink_count as f32).sqrt();
            for i in 0..frames {
                left[i] *= gain;
                right[i] *= gain;
            }
        }

        // Hand the mixed block to the master observers. Lock-free and
        // allocation-free.
        self.master
            .observe(left, right, frames, self.current_sample);

        self.store_carry(frames);
    }

    /// Processes a single acyclic module over frames `from..to` of the
    /// segment (all of it but where a request splits it, see
    /// [`Self::process_conditioned`]), as a block of its own.
    pub(super) fn process_span(&mut self, module_idx: usize, from: usize, to: usize) {
        let frames = to - from;
        // Zero connected input ports, then sum every route feeding them.
        let port_count = self.connected_in_ports[module_idx].len();
        for pi in 0..port_count {
            let port = self.connected_in_ports[module_idx][pi];
            if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
                inst.module_mut().input_block_mut(port)[..frames].fill(0.0);
            }
        }

        let route_count = self.compiled_routes[module_idx].len();
        for r in 0..route_count {
            let route = self.compiled_routes[module_idx][r];
            let base = route.from_port * self.block_capacity + from;
            if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
                let dst = inst.module_mut().input_block_mut(route.to_port);
                let src = &self.out_bufs[route.from_module][base..base + frames];
                for k in 0..frames {
                    dst[k] += src[k];
                }
            }
        }

        if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
            // A span after the first starts at its own frame 0.
            let _frame_zero = (from > 0).then(|| FrameScope::frame(from));
            take_automation(inst.module_mut());
            inst.module_mut().process(frames);
        }

        let n_out = self.out_counts[module_idx];
        for p in 0..n_out {
            let base = p * self.block_capacity + from;
            if let Some((_, inst)) = self.modules.get_index(module_idx) {
                let src = inst.module().output_block(p);
                self.out_bufs[module_idx][base..base + frames].copy_from_slice(&src[..frames]);
            }
        }
    }

    /// Processes a feedback group sample-by-sample so back-edges observe a
    /// one-sample delay, preserving feedback fidelity regardless of block size.
    fn process_feedback_group(&mut self, group: usize, frames: usize) {
        for s in 0..frames {
            let _frame_zero = FrameScope::frame(s);
            let member_count = self.process_groups[group].members.len();
            for mi in 0..member_count {
                let module_idx = self.process_groups[group].members[mi];

                // Zero connected input ports at frame 0, then sum routes.
                let port_count = self.connected_in_ports[module_idx].len();
                for pi in 0..port_count {
                    let port = self.connected_in_ports[module_idx][pi];
                    if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
                        inst.module_mut().input_block_mut(port)[0] = 0.0;
                    }
                }

                let route_count = self.compiled_routes[module_idx].len();
                for r in 0..route_count {
                    let route = self.compiled_routes[module_idx][r];
                    let value = if route.delayed {
                        if s == 0 {
                            self.out_prev[route.from_module][route.from_port]
                        } else {
                            self.out_bufs[route.from_module]
                                [route.from_port * self.block_capacity + (s - 1)]
                        }
                    } else {
                        self.out_bufs[route.from_module][route.from_port * self.block_capacity + s]
                    };
                    if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
                        inst.module_mut().input_block_mut(route.to_port)[0] += value;
                    }
                }

                if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
                    take_automation(inst.module_mut());
                    inst.module_mut().process(1);
                }

                let n_out = self.out_counts[module_idx];
                for p in 0..n_out {
                    let value = self
                        .modules
                        .get_index(module_idx)
                        .map(|(_, inst)| inst.module().output_block(p)[0])
                        .unwrap_or(0.0);
                    self.out_bufs[module_idx][p * self.block_capacity + s] = value;
                }
            }
        }
    }

    /// Records the final sample of each module output port for the next block's
    /// frame-0 delayed feedback reads.
    fn store_carry(&mut self, frames: usize) {
        let n = self.modules.len();
        for m in 0..n {
            let n_out = self.out_counts[m];
            for p in 0..n_out {
                self.out_prev[m][p] = self.out_bufs[m][p * self.block_capacity + (frames - 1)];
            }
        }
    }
}
