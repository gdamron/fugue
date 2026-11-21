//! Cables patched into an audio input that ramp in instead of stepping in.
//!
//! An edit that patches a sounding source into an audio input would step
//! what its module hears, from what the input carried to that plus the new
//! cable's signal: a click. So the graph ramps each such cable in, alone,
//! from silence over [`RELEASE_SECONDS`]: the input hears the routes it
//! had, and the new cable scaled by a gain rising linearly from 0 to 1. A
//! cable the same edit unpatches from such a port falls the same way, its
//! source still playing, so a swap or a module inserted into a chain
//! crossfades. A falling cable whose source is gone, or no longer runs
//! ahead of the input's module, glides out from what it last carried
//! instead (see [`super::releases`]). Only inputs whose module says they
//! fade in do (see [`crate::Module::input_fades_in`]): a gate, a clock or
//! a pitch takes its new cable at once, as before.
//!
//! Onsets live in a fixed set of slots, as releases do, so starting,
//! playing and ending one never allocates; a cable past them steps.

use super::releases::{id_at, Previous, RELEASE_SECONDS};
use super::{CompiledRoute, SignalGraph};
use indexmap::IndexMap;

use crate::invention::runtime::ModuleInstance;

/// The most cables ramping in at once.
const MAX_ONSETS: usize = 64;

/// The most ports unpatched where their module plays the departure, at
/// one sample.
const MAX_DEPARTED: usize = 16;

/// The ramp's length before a sample rate is known: 10 ms at 48 kHz.
const DEFAULT_FRAMES: u64 = 480;

/// One cable ramping into or out of one input.
#[derive(Clone, Copy, Default)]
struct Onset {
    module: usize,
    port: usize,
    /// The cable's source module, in the graph's order, and output port.
    source: (usize, usize),
    /// The sample the ramp started at.
    start: u64,
    /// Whether `source` indexes the installed topology's module order:
    /// false between an install's module remap and its source remap.
    current: bool,
    /// Ramping out, from `level`, rather than in from 0.
    falling: bool,
    /// A falling cable's gain as it began to fall.
    level: f32,
}

impl Onset {
    /// The cable's gain at `sample` of a ramp `frames` long: one step of it
    /// already taken at its first sample, at its end from its last on.
    #[inline]
    fn gain(&self, sample: u64, frames: u64) -> f32 {
        let taken = (sample.saturating_sub(self.start) + 1).min(frames);
        let risen = taken as f32 / frames as f32;
        if self.falling {
            self.level * (1.0 - risen)
        } else {
            risen
        }
    }

    /// What of the cable the input last heard by sample `now`.
    #[inline]
    fn heard(&self, now: u64, frames: u64) -> f32 {
        match (now > self.start, self.falling) {
            (true, _) => self.gain(now - 1, frames),
            (false, true) => self.level,
            (false, false) => 0.0,
        }
    }
}

/// The cables ramping into a graph's inputs.
pub(crate) struct Onsets {
    slots: [Onset; MAX_ONSETS],
    len: usize,
    frames: u64,
    /// The ports (module, port) unpatched at sample `departed_at` whose
    /// module plays the departure itself (an input without a rest, see
    /// [`crate::Module::input_rest`]).
    departed: [(usize, usize); MAX_DEPARTED],
    departed_len: usize,
    departed_at: u64,
}

impl Default for Onsets {
    fn default() -> Self {
        Self {
            slots: [Onset::default(); MAX_ONSETS],
            len: 0,
            frames: DEFAULT_FRAMES,
            departed: [(0, 0); MAX_DEPARTED],
            departed_len: 0,
            departed_at: 0,
        }
    }
}

impl Onsets {
    /// Times onsets for `sample_rate`, as releases are.
    pub(super) fn set_sample_rate(&mut self, sample_rate: u32) {
        self.frames = ((sample_rate as f32 * RELEASE_SECONDS) as u64).max(1);
    }

    #[inline]
    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Starts ramping the cable from `source` into `port` of `module` at
    /// sample `now`, or reports there is no room.
    fn begin(&mut self, module: usize, port: usize, source: (usize, usize), now: u64) -> bool {
        self.push(Onset {
            module,
            port,
            source,
            start: now,
            current: true,
            falling: false,
            level: 0.0,
        })
    }

    /// Starts ramping the cable from `source` out of `port` of `module` at
    /// sample `now`, from `level` of it, or reports there is no room.
    fn fall(
        &mut self,
        module: usize,
        port: usize,
        source: (usize, usize),
        level: f32,
        now: u64,
    ) -> bool {
        self.push(Onset {
            module,
            port,
            source,
            start: now,
            current: true,
            falling: true,
            level,
        })
    }

    fn push(&mut self, onset: Onset) -> bool {
        if self.len == MAX_ONSETS {
            return false;
        }
        self.slots[self.len] = onset;
        self.len += 1;
        true
    }

    /// Ends the ramp of one cable from `source` (in the order the onset's
    /// source is in) into `port` of `module`, returning what of it the
    /// input last heard by `now`: 1 for a cable not ramping.
    pub(super) fn take(
        &mut self,
        module: usize,
        port: usize,
        source: (usize, usize),
        now: u64,
    ) -> f32 {
        let found = (0..self.len).find(|&i| {
            let onset = self.slots[i];
            !onset.falling && onset.module == module && onset.port == port && onset.source == source
        });
        match found {
            Some(i) => {
                let heard = self.slots[i].heard(now, self.frames);
                self.swap_remove(i);
                heard
            }
            None => 1.0,
        }
    }

    /// Forgets every ramp into or out of `port` of `module`, returning what
    /// the input last heard by `now` of the cables falling out of it, by
    /// what each last carried in `carry` (in the order their sources are).
    pub(super) fn cancel_port(
        &mut self,
        module: usize,
        port: usize,
        carry: &[Vec<f32>],
        now: u64,
    ) -> f32 {
        let mut falling = 0.0;
        let mut i = 0;
        while i < self.len {
            let onset = self.slots[i];
            if onset.module == module && onset.port == port {
                if onset.falling {
                    falling += carried(carry, onset.source) * onset.heard(now, self.frames);
                }
                self.swap_remove(i);
            } else {
                i += 1;
            }
        }
        falling
    }

    /// Notes that `port` of `module` was unpatched at sample `now` and its
    /// module plays the departure. Past the room, it is not noted.
    pub(super) fn depart(&mut self, module: usize, port: usize, now: u64) {
        if now != self.departed_at {
            self.departed_len = 0;
            self.departed_at = now;
        }
        if self.departed_len < MAX_DEPARTED {
            self.departed[self.departed_len] = (module, port);
            self.departed_len += 1;
        }
    }

    /// Whether `port` of `module` was unpatched at sample `now`, nothing
    /// having played since, with its module playing the departure: patched
    /// again, the module carries on from where it was, as if never
    /// unpatched, so a ramp from silence would dip.
    fn departed(&self, module: usize, port: usize, now: u64) -> bool {
        now == self.departed_at && self.departed[..self.departed_len].contains(&(module, port))
    }

    /// Moves every onset to its module's index in a new module order,
    /// dropping those of modules `new_of` does not carry over. Their
    /// sources stay in the old order until [`Self::remap_sources`].
    pub(super) fn remap_modules(&mut self, new_of: impl Fn(usize) -> Option<usize>) {
        let mut d = 0;
        while d < self.departed_len {
            match new_of(self.departed[d].0) {
                Some(new) => {
                    self.departed[d].0 = new;
                    d += 1;
                }
                None => {
                    self.departed_len -= 1;
                    self.departed[d] = self.departed[self.departed_len];
                }
            }
        }
        let mut i = 0;
        while i < self.len {
            match new_of(self.slots[i].module) {
                Some(new) => {
                    self.slots[i].module = new;
                    self.slots[i].current = false;
                    i += 1;
                }
                None => self.swap_remove(i),
            }
        }
    }

    /// Moves each onset's source to where `source_of` puts it, dropping
    /// those whose source is gone.
    pub(super) fn remap_sources(&mut self, source_of: impl Fn(usize) -> Option<usize>) {
        let mut i = 0;
        while i < self.len {
            let onset = &mut self.slots[i];
            if onset.current {
                i += 1;
                continue;
            }
            match source_of(onset.source.0) {
                Some(from) => {
                    onset.source.0 = from;
                    onset.current = true;
                    i += 1;
                }
                None => self.swap_remove(i),
            }
        }
    }

    /// Forgets every onset.
    pub(super) fn clear(&mut self) {
        self.len = 0;
        self.departed_len = 0;
    }

    fn swap_remove(&mut self, i: usize) {
        self.len -= 1;
        self.slots[i] = self.slots[self.len];
    }
}

/// What output `port` of module `source` last carried, by `carry`.
fn carried(carry: &[Vec<f32>], (source, port): (usize, usize)) -> f32 {
    carry
        .get(source)
        .and_then(|ports| ports.get(port))
        .copied()
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
}

impl SignalGraph {
    /// Hands each falling cable whose source is gone, or no longer runs
    /// ahead of its input's module, to a glide out from what it last
    /// carried by `carry`, and ends the ramp of each rising one whose route
    /// now reads another frame than its ramp would (a back edge), so it is
    /// heard whole. The onsets' sources are in the order of `retired`, the
    /// modules an install replaced, or the graph's own order without one.
    /// Before an install resets any input.
    pub(super) fn settle_falling(
        &mut self,
        carry: &[Vec<f32>],
        retired: Option<&IndexMap<String, ModuleInstance>>,
    ) {
        let now = self.current_sample;
        let mut i = 0;
        while i < self.onsets.len {
            let onset = self.onsets.slots[i];
            let source = match retired {
                Some(retired) => moved_to(retired, &self.modules, onset.source.0),
                None => Some(onset.source.0),
            };
            let (groups, group_of) = (&self.process_groups, &self.group_of);
            let kept = match (onset.falling, source) {
                (true, Some(source)) => runs_ahead(groups, group_of, source, onset.module),
                (true, None) => false,
                // A rising cable unpatched, or whose source is gone, is
                // lost with its route (see `take`).
                (false, Some(source)) => {
                    let routed = self
                        .compiled_routes
                        .get(onset.module)
                        .is_some_and(|routes| {
                            routes.iter().any(|route| {
                                route.to_port == onset.port
                                    && (route.from_module, route.from_port)
                                        == (source, onset.source.1)
                            })
                        });
                    !routed || reads_alongside(groups, group_of, source, onset.module)
                }
                (false, None) => true,
            };
            if kept {
                i += 1;
                continue;
            }
            if !onset.falling {
                self.onsets.swap_remove(i);
                continue;
            }
            let from = carried(carry, onset.source) * onset.heard(now, self.onsets.frames);
            self.onsets.swap_remove(i);
            self.glide_out(onset.module, onset.port, from);
        }
    }

    /// Whether input `port` of module `mi` fades in.
    pub(super) fn fades_in(&self, mi: usize, port: usize) -> bool {
        self.modules
            .get_index(mi)
            .is_some_and(|(_, inst)| inst.module().input_fades_in(port))
    }

    /// Ramps in each cable into input `port` of module `mi` that
    /// `previous`, its connections before the edit, lacks, when the port
    /// fades in. A cable is its source module (by id, or by index when
    /// `previous` is in the graph's own order) and port, and one made twice
    /// counts twice. Returns whether any new cable steps in instead: the
    /// port does not fade in, there is no room, or it is a back edge.
    pub(super) fn ramp_in(&mut self, mi: usize, port: usize, previous: Option<Previous>) -> bool {
        // Before the first sample nothing has been heard to step from, and
        // a port its module is still to play a departure from carries on.
        let now_sample = self.current_sample;
        let fades = self.fades_in(mi, port)
            && now_sample > 0
            && !self.onsets.departed(mi, port, now_sample);
        let now: &[CompiledRoute] = self.compiled_routes.get(mi).map_or(&[], Vec::as_slice);
        let into = |route: &&CompiledRoute| route.to_port == port;
        let before = previous.map_or(&[][..], |previous| previous.routes);
        let modules = &self.modules;
        let same = |old: &CompiledRoute, new: &CompiledRoute| {
            let source = match previous.and_then(|previous| previous.modules) {
                None => Some(old.from_module),
                Some(retired) => moved_to(retired, modules, old.from_module),
            };
            old.from_port == new.from_port && source == Some(new.from_module)
        };
        let mut stepped = false;
        for (k, new) in now.iter().filter(into).enumerate() {
            let twin = |r: &&CompiledRoute| {
                r.from_module == new.from_module && r.from_port == new.from_port
            };
            let rank = now.iter().filter(into).take(k).filter(twin).count();
            let had = before
                .iter()
                .filter(into)
                .filter(|old| same(old, new))
                .count();
            if rank < had {
                continue;
            }
            // A back edge reads another frame than the ramp would play.
            let alongside =
                reads_alongside(&self.process_groups, &self.group_of, new.from_module, mi);
            if !fades || !alongside {
                stepped = true;
                continue;
            }
            let source = (new.from_module, new.from_port);
            stepped |= !self.onsets.begin(mi, port, source, now_sample);
        }
        stepped
    }

    /// Plays frames `from..from + frames` of the segment of each cable
    /// ramping into or out of module `module`'s inputs, at its frame 0: the
    /// routes summed a rising cable whole, so what it is not yet heard at
    /// comes off; a falling cable is no route, so what it is still heard at
    /// goes on. A port's ramps are summed before they reach its input, so
    /// two cables carrying the same signal, one rising as the other falls,
    /// cancel exactly and the input is the routes' own sum, bit for bit.
    pub(super) fn play_onsets(&mut self, module: usize, from: usize, frames: usize) {
        let first = self.segment.start + from as u64;
        let length = self.onsets.frames;
        let (slots, len) = (&self.onsets.slots, self.onsets.len);
        let ours = |j: usize, port: usize| slots[j].module == module && slots[j].port == port;
        let Some((_, inst)) = self.modules.get_index_mut(module) else {
            return;
        };
        for i in 0..len {
            let port = slots[i].port;
            // Each port once, from its first ramp.
            if slots[i].module != module || (0..i).any(|j| ours(j, port)) {
                continue;
            }
            let input = inst.module_mut().input_block_mut(port);
            for (k, x) in input[..frames].iter_mut().enumerate() {
                let sample = first + k as u64;
                let mut ramps = 0.0;
                for onset in slots[i..len]
                    .iter()
                    .filter(|o| o.module == module && o.port == port)
                {
                    let (source, out) = onset.source;
                    let at = out * self.block_capacity + from + k;
                    let Some(&value) = self.out_bufs.get(source).and_then(|b| b.get(at)) else {
                        continue;
                    };
                    // A rising cable is in the routes' sum, a falling one not.
                    let gain = onset.gain(sample, length);
                    ramps += value * if onset.falling { gain } else { gain - 1.0 };
                }
                *x += ramps;
            }
        }
    }

    /// Ends each ramp that has played out by the end of the block.
    pub(super) fn finish_onsets(&mut self) {
        let mut i = 0;
        while i < self.onsets.len {
            let onset = self.onsets.slots[i];
            if self.current_sample >= onset.start + self.onsets.frames {
                self.onsets.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }
}

/// The index, in `modules`, of the module at `index` of `retired`.
pub(super) fn moved_to(
    retired: &IndexMap<String, ModuleInstance>,
    modules: &IndexMap<String, ModuleInstance>,
    index: usize,
) -> Option<usize> {
    id_at(retired, index).and_then(|id| modules.get_index_of(id))
}

mod order;

use order::{reads_alongside, runs_ahead};

#[cfg(test)]
mod tests;
