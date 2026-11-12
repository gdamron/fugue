//! Audio-thread block processing for the [`SignalGraph`].

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

use super::conditions::Segment;
use super::SignalGraph;
use crate::audio_thread::AudioThreadScope;
use crate::control_request::{take_automation, FrameScope};
use crate::flush_to_zero::FlushToZeroScope;
use crate::payload::RenderScope;

impl SignalGraph {
    /// Processes a block of `frames` frames (`frames == left.len() == right.len()`,
    /// always `<= block_size`), mixing all sink outputs into `left`/`right`.
    ///
    /// Process groups run in topological order: acyclic modules a whole block at
    /// a time, feedback cycles in chunks of their shortest declared delay
    /// (see [`super::chunks`]) or else sample-by-sample. Zero heap
    /// allocations.
    ///
    /// The block runs as segments split at each pending control request's
    /// sample (see [`super::requests`]); with none due it is one segment.
    /// All of it runs inside an [`AudioThreadScope`], so debug builds catch
    /// a payload dropped, or control-only code reached, from here, and
    /// inside a [`FlushToZeroScope`], so no module spends the block on
    /// subnormal numbers. Every live backend and every offline render
    /// renders through here.
    ///
    /// A zero-length block takes up publications, input writes and requests
    /// as any block start does and applies the requests due at the current
    /// sample, but runs no module and advances no time: how a backend with
    /// no clock settles what was submitted (`NullBackend`). Device streams
    /// and offline renders never ask for one.
    ///
    /// A paused graph plays out its fade and then holds (see
    /// [`super::pause`]): each block it holds runs as a zero-length block
    /// does and renders silence.
    ///
    /// A panic in a module fails the graph on its way out, keeping all it
    /// held for teardown (see [`super::unwind`]): every later block renders
    /// silence and touches nothing.
    pub(crate) fn process_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        let _audio_thread = AudioThreadScope::enter();
        let _flush_to_zero = FlushToZeroScope::enter();
        let _rendering = RenderScope::enter();
        if self.failed {
            left.fill(0.0);
            right.fill(0.0);
            return;
        }
        let rendered = catch_unwind(AssertUnwindSafe(|| self.render_block(left, right)));
        if let Err(panic) = rendered {
            self.fail();
            self.keep_caught();
            resume_unwind(panic);
        }
    }

    /// [`Self::process_block`] on a graph that has not failed.
    fn render_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        // Read before the requests are taken, so a write submitted before a
        // pause or resume is taken in the block that sees it: a write made
        // while held and then resumed applies before the first frame plays.
        let paused = self.transport.pause_requested();
        self.ensure_process_order();

        let frames = left.len().min(right.len()).min(self.block_capacity);
        // A paused graph plays only what is left of its fade out, and holds
        // as a zero-length block once it has faded: silence for the rest.
        let frames = self.pause.frames_to_play(paused, frames);
        left[frames..].fill(0.0);
        right[frames..].fill(0.0);
        if frames == 0 {
            self.apply_due_requests(0);
        }
        let mut start = 0;
        while start < frames {
            let end = start + self.apply_due_requests(frames - start);
            self.process_segment(&mut left[start..end], &mut right[start..end]);
            self.tap_notes(end - start);
            self.tap_outputs(end - start);
            if let Some(watch) = &mut self.end_watch {
                watch.record(&self.out_bufs, self.block_capacity, end - start);
            }
            start = end;
        }
        if !self.releases.is_empty() {
            self.finish_releases();
        }
        // Every module has processed; what a zero-length block, or a module
        // that did not, left staged applies before the block is read back.
        self.flush_all_staged();
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
                let chunk = self.process_groups[g].chunks.as_ref().map(|c| c.frames);
                if chunk == Some(1) {
                    self.process_unit_chunks(g, frames);
                } else if chunk.is_some() {
                    self.process_chunked_group(g, frames);
                } else {
                    self.process_feedback_group(g, frames);
                }
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
        let mut non_finite_samples = 0;
        for si in 0..sink_count {
            let sink_idx = self.sink_indices[si];
            if let Some((_, inst)) = self.modules.get_index(sink_idx) {
                if let Some((l, r)) = inst.sink_block() {
                    for i in 0..frames {
                        left[i] += l[i];
                        right[i] += r[i];
                    }
                }
                non_finite_samples += inst.non_finite_samples();
            }
        }
        if sink_count > 1 {
            let gain = 1.0 / (sink_count as f32).sqrt();
            for i in 0..frames {
                left[i] *= gain;
                right[i] *= gain;
            }
        }

        // Fade it while a pause or resume fades, before the observers read
        // it, so they read what is heard.
        self.pause.fade(&mut left[..frames], &mut right[..frames]);

        // Hand the mixed block to the master observers. Lock-free and
        // allocation-free.
        self.master
            .observe(left, right, frames, self.current_sample, non_finite_samples);

        self.store_carry(frames);
    }

    /// Processes a single acyclic module over frames `from..to` of the
    /// segment (all of it but where a request splits it, see
    /// [`Self::process_conditioned`]), as a block of its own.
    pub(super) fn process_span(&mut self, module_idx: usize, from: usize, to: usize) {
        let frames = to - from;
        self.gather_inputs(module_idx, from, frames);

        // Its requests' writes, then the automation written after them.
        self.flush_staged_on(module_idx);
        if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
            // A span after the first starts at its own frame 0.
            let _frame_zero = (from > 0).then(|| FrameScope::frame(from));
            take_automation(inst.module_mut());
            inst.module_mut().process(frames);
        }
        self.store_outputs(module_idx, from, frames);
    }

    /// Runs a feedback group in chunks (see [`super::chunks`]): for each,
    /// the cut members emit from their history, the rest process over the
    /// chunk in order, and the cuts absorb what the rest wrote. Every route
    /// inside the group reads the same chunk, so the loop is exactly as
    /// long as its delays.
    fn process_chunked_group(&mut self, group: usize, frames: usize) {
        let Some(chunk) = self.process_groups[group].chunks.as_ref().map(|c| c.frames) else {
            return;
        };
        let mut from = 0;
        while from < frames {
            let to = (from + chunk).min(frames);
            let n = to - from;
            let cut_count = self.process_groups[group]
                .chunks
                .as_ref()
                .map_or(0, |c| c.cuts.len());
            for c in 0..cut_count {
                let module_idx = self.chunk_member(group, c, true);
                if self.emit_reads(group, c) {
                    self.gather_inputs(module_idx, from, n);
                }
                self.flush_staged_on(module_idx);
                if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
                    let _frame_zero = (from > 0).then(|| FrameScope::frame(from));
                    take_automation(inst.module_mut());
                    inst.module_mut().emit(n);
                }
                self.store_outputs(module_idx, from, n);
            }
            let rest_count = self.process_groups[group]
                .chunks
                .as_ref()
                .map_or(0, |c| c.rest.len());
            for r in 0..rest_count {
                let module_idx = self.chunk_member(group, r, false);
                self.process_span(module_idx, from, to);
            }
            for c in 0..cut_count {
                let module_idx = self.chunk_member(group, c, true);
                self.gather_inputs(module_idx, from, n);
                if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
                    inst.module_mut().absorb(n);
                }
            }
            from = to;
        }
    }

    /// Whether the `k`th cut of chunked group `group` reads an input as it
    /// emits, which is then gathered first.
    fn emit_reads(&self, group: usize, k: usize) -> bool {
        let chunks = self.process_groups[group].chunks.as_ref();
        chunks
            .and_then(|c| c.emit_reads.get(k))
            .copied()
            .unwrap_or(true)
    }

    /// The `k`th cut (`cut`) or other member of chunked group `group`.
    fn chunk_member(&self, group: usize, k: usize, cut: bool) -> usize {
        let chunks = self.process_groups[group].chunks.as_ref();
        chunks
            .and_then(|c| if cut { c.cuts.get(k) } else { c.rest.get(k) })
            .copied()
            .unwrap_or(usize::MAX)
    }

    /// Fills module `module_idx`'s connected inputs with frames
    /// `from..from + frames` of the routes feeding them, at its frame 0.
    fn gather_inputs(&mut self, module_idx: usize, from: usize, frames: usize) {
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
        if !self.releases.is_empty() {
            self.play_releases(module_idx, from, frames);
        }
    }

    /// Stores `frames` of module `module_idx`'s outputs at frame `from` of
    /// its output buffers.
    fn store_outputs(&mut self, module_idx: usize, from: usize, frames: usize) {
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
                self.step(module_idx, s, Step::Process, true);
            }
        }
    }

    /// Runs a chunked group whose chunks are a single frame as
    /// [`Self::process_chunked_group`] would, a frame at a time as
    /// [`Self::process_feedback_group`] does, without its spans' overhead.
    fn process_unit_chunks(&mut self, group: usize, frames: usize) {
        let (cuts, rest) = self.process_groups[group]
            .chunks
            .as_ref()
            .map_or((0, 0), |c| (c.cuts.len(), c.rest.len()));
        for s in 0..frames {
            let _frame_zero = FrameScope::frame(s);
            for c in 0..cuts {
                let module_idx = self.chunk_member(group, c, true);
                self.step(module_idx, s, Step::Emit(self.emit_reads(group, c)), false);
            }
            for r in 0..rest {
                let module_idx = self.chunk_member(group, r, false);
                self.step(module_idx, s, Step::Process, false);
            }
            for c in 0..cuts {
                let module_idx = self.chunk_member(group, c, true);
                self.step(module_idx, s, Step::Absorb, false);
            }
        }
    }

    /// Runs module `module_idx` for frame `s` of a group run a frame at a
    /// time: its routes summed into its inputs' frame 0 (a back-edge
    /// reading the frame before when `delays`), then `step`.
    fn step(&mut self, module_idx: usize, s: usize, step: Step, delays: bool) {
        if !matches!(step, Step::Emit(false)) {
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
                let value = if route.delayed && delays {
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
            if !self.releases.is_empty() {
                self.play_releases(module_idx, s, 1);
            }
        }

        if step == Step::Absorb {
            if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
                inst.module_mut().absorb(1);
            }
            return;
        }
        self.flush_staged_on(module_idx);
        if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
            take_automation(inst.module_mut());
            if step == Step::Process {
                inst.module_mut().process(1);
            } else {
                inst.module_mut().emit(1);
            }
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

/// What [`SignalGraph::step`] has a module do with its frame.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Process,
    /// Emit, its inputs gathered first when it reads any as it emits.
    Emit(bool),
    Absorb,
}
