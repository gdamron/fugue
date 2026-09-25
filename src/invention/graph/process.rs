//! Audio-thread block processing for the [`SignalGraph`].

use super::SignalGraph;

impl SignalGraph {
    /// Processes a block of `frames` frames (`frames == left.len() == right.len()`,
    /// always `<= block_size`), mixing all sink outputs into `left`/`right`.
    ///
    /// Process groups run in topological order: acyclic modules a whole block at
    /// a time, feedback cycles sample-by-sample. Zero heap allocations.
    pub(crate) fn process_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.ensure_process_order();

        let frames = left.len().min(right.len());
        if frames == 0 {
            return;
        }
        let frames = frames.min(self.block_capacity);
        self.current_sample += frames as u64;

        for g in 0..self.process_groups.len() {
            if self.process_groups[g].feedback {
                self.process_feedback_group(g, frames);
            } else {
                let module_idx = self.process_groups[g].members[0];
                self.process_full_block(module_idx, frames);
            }
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

        // Fold this block's master peak into the meter. Lock-free and
        // allocation-free: one pass over buffers already in hand, then two
        // atomic stores (FUG-239 #5).
        let mut left_peak = 0.0f32;
        let mut right_peak = 0.0f32;
        for i in 0..frames {
            left_peak = left_peak.max(left[i].abs());
            right_peak = right_peak.max(right[i].abs());
        }
        self.master_peak.observe(left_peak, right_peak);

        // Hand the same block to the spectrum tap. Lock-free and
        // allocation-free, and a single relaxed load when nobody is watching.
        self.master_spectrum.observe_block(left, right, frames);

        self.store_carry(frames);
    }

    /// Processes a single acyclic module a whole block at a time.
    fn process_full_block(&mut self, module_idx: usize, frames: usize) {
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
            let base = route.from_port * self.block_capacity;
            if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
                let dst = inst.module_mut().input_block_mut(route.to_port);
                let src = &self.out_bufs[route.from_module][base..base + frames];
                for k in 0..frames {
                    dst[k] += src[k];
                }
            }
        }

        if let Some((_, inst)) = self.modules.get_index_mut(module_idx) {
            inst.module_mut().process(frames);
        }

        let n_out = self.out_counts[module_idx];
        for p in 0..n_out {
            let base = p * self.block_capacity;
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
