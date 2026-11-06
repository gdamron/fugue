//! Unpatched inputs that glide to rest instead of stepping there.
//!
//! An edit that unpatches an input would step what its module hears, from
//! the value the connection last carried to the one an unconnected input
//! holds: a click on audio, a jump on a modulated parameter. So the graph
//! holds such an input where the connection left it, still declared
//! connected, and glides it over [`RELEASE_SECONDS`] to where its module
//! says it rests (see [`crate::Module::input_rest`]), then lets it go as
//! before: cleared and declared unconnected. A port patched again before
//! then is the connection's once more.
//!
//! Releases live in a fixed set of slots, so starting, playing and ending
//! one never allocates; a release past them lets its input go at once.

use super::{CompiledRoute, SignalGraph};

/// How long an unpatched input takes to come to rest.
pub(crate) const RELEASE_SECONDS: f32 = 0.01;

/// The most inputs releasing at once.
const MAX_RELEASES: usize = 64;

/// The release length before a sample rate is known: 10 ms at 48 kHz.
const DEFAULT_FRAMES: u64 = 480;

/// One unpatched input on its way to rest.
#[derive(Clone, Copy, Default)]
struct Release {
    module: usize,
    port: usize,
    from: f32,
    to: f32,
    /// The sample the release started at.
    start: u64,
}

impl Release {
    /// The input's value at `sample` of a release `frames` long: one step of
    /// the glide already taken at its first sample, `to` from its last on.
    #[inline]
    fn value(&self, sample: u64, frames: u64) -> f32 {
        let taken = (sample.saturating_sub(self.start) + 1).min(frames);
        self.from + (self.to - self.from) * (taken as f32 / frames as f32)
    }
}

/// The inputs releasing in a graph.
pub(crate) struct Releases {
    slots: [Release; MAX_RELEASES],
    len: usize,
    frames: u64,
}

impl Default for Releases {
    fn default() -> Self {
        Self {
            slots: [Release::default(); MAX_RELEASES],
            len: 0,
            frames: DEFAULT_FRAMES,
        }
    }
}

impl Releases {
    /// Times releases for `sample_rate`.
    fn set_sample_rate(&mut self, sample_rate: u32) {
        self.frames = ((sample_rate as f32 * RELEASE_SECONDS) as u64).max(1);
    }

    #[inline]
    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Starts releasing `port` of `module` from `from` to `to` at sample
    /// `start`, or reports there is no room.
    fn begin(&mut self, module: usize, port: usize, from: f32, to: f32, start: u64) -> bool {
        if self.len == MAX_RELEASES || !from.is_finite() || !to.is_finite() {
            return false;
        }
        self.slots[self.len] = Release {
            module,
            port,
            from,
            to,
            start,
        };
        self.len += 1;
        true
    }

    /// Forgets the release of `port` of `module`, reporting whether there
    /// was one.
    pub(super) fn cancel(&mut self, module: usize, port: usize) -> bool {
        let found = (0..self.len).find(|&i| {
            let release = self.slots[i];
            release.module == module && release.port == port
        });
        found.inspect(|&i| self.swap_remove(i)).is_some()
    }

    /// Moves every release to its module's index in a new module order,
    /// dropping those of modules `new_of` does not carry over.
    pub(super) fn remap(&mut self, new_of: impl Fn(usize) -> Option<usize>) {
        let mut i = 0;
        while i < self.len {
            match new_of(self.slots[i].module) {
                Some(new) => {
                    self.slots[i].module = new;
                    i += 1;
                }
                None => self.swap_remove(i),
            }
        }
    }

    /// Forgets every release.
    pub(super) fn clear(&mut self) {
        self.len = 0;
    }

    fn swap_remove(&mut self, i: usize) {
        self.len -= 1;
        self.slots[i] = self.slots[self.len];
    }
}

/// A module's connections before an edit: the ports it had connected, the
/// routes into them, and every module's last outputs as of then (indexed as
/// those routes are).
#[derive(Clone, Copy)]
pub(super) struct Previous<'a> {
    pub(super) ports: &'a [usize],
    pub(super) routes: &'a [CompiledRoute],
    pub(super) carry: &'a [Vec<f32>],
}

impl Previous<'_> {
    /// What the routes into `port` last summed to, if every one is known.
    fn held(&self, port: usize) -> Option<f32> {
        let mut sum = 0.0;
        for route in self.routes.iter().filter(|route| route.to_port == port) {
            sum += *self.carry.get(route.from_module)?.get(route.from_port)?;
        }
        Some(sum)
    }
}

impl SignalGraph {
    /// Times the graph's input releases for `sample_rate`.
    pub(crate) fn set_sample_rate(&mut self, sample_rate: u32) {
        self.releases.set_sample_rate(sample_rate);
    }

    /// Starts releasing input `port` of module `mi`, just unpatched, from
    /// where `previous` left it; or reports that it lets go at once (its
    /// module says so, the value is unknown, or there is no room).
    pub(super) fn release_input(&mut self, mi: usize, port: usize, previous: Previous) -> bool {
        let Some((_, inst)) = self.modules.get_index(mi) else {
            return false;
        };
        let (Some(from), Some(to)) = (previous.held(port), inst.module().input_rest(port)) else {
            return false;
        };
        self.releases.begin(mi, port, from, to, self.current_sample)
    }

    /// Writes frames `from..from + frames` of the segment into the inputs
    /// of module `module` that are releasing, at its frame 0, gliding each
    /// to where its module says it rests as of this span.
    pub(super) fn play_releases(&mut self, module: usize, from: usize, frames: usize) {
        let first = self.segment.start + from as u64;
        let length = self.releases.frames;
        for i in 0..self.releases.len {
            let release = self.releases.slots[i];
            if release.module != module {
                continue;
            }
            if let Some((_, inst)) = self.modules.get_index_mut(module) {
                // A control written since the release began (as a reload's
                // are, after its edit) moves where it rests: it glides on
                // from where it is to there, a release's length from now.
                let rest = inst.module().input_rest(release.port);
                let rest = rest.filter(|to| to.is_finite()).unwrap_or(release.to);
                let release = if rest == release.to {
                    release
                } else {
                    let now = first.max(release.start);
                    let at = release.value(now.saturating_sub(1), length);
                    let from = if now == release.start {
                        release.from
                    } else {
                        at
                    };
                    let moved = Release {
                        from,
                        to: rest,
                        start: now,
                        ..release
                    };
                    self.releases.slots[i] = moved;
                    moved
                };
                let input = inst.module_mut().input_block_mut(release.port);
                for (k, x) in input[..frames].iter_mut().enumerate() {
                    *x = release.value(first + k as u64, length);
                }
            }
        }
    }

    /// Lets go of each input whose release has played out by the end of the
    /// block: cleared and declared unconnected, as an unpatched input is.
    pub(super) fn finish_releases(&mut self) {
        let mut i = 0;
        while i < self.releases.len {
            let release = self.releases.slots[i];
            if self.current_sample < release.start + self.releases.frames {
                i += 1;
                continue;
            }
            if let Some((_, inst)) = self.modules.get_index_mut(release.module) {
                let module = inst.module_mut();
                module.input_block_mut(release.port).fill(0.0);
                module.set_input_connected(release.port, false);
            }
            self.releases.swap_remove(i);
        }
    }
}

#[cfg(test)]
mod tests;
