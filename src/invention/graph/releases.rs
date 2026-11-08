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
//! An input summed from several cables that loses only some of them would
//! step the same way, from the old sum to the new one. So each cable it
//! loses leaves a *share*: what that cable last carried, added to what the
//! remaining routes carry and gliding to 0 over the same length. The input
//! stays the routes' throughout and glides from the old sum to the new.
//!
//! Releases live in a fixed set of slots, so starting, playing and ending
//! one never allocates; a release past them lets its input go at once.

use indexmap::IndexMap;

use super::{CompiledRoute, SignalGraph};
use crate::invention::runtime::ModuleInstance;

/// How long an unpatched input takes to come to rest.
pub(crate) const RELEASE_SECONDS: f32 = 0.01;

/// The most inputs releasing at once.
const MAX_RELEASES: usize = 64;

/// The release length before a sample rate is known: 10 ms at 48 kHz.
const DEFAULT_FRAMES: u64 = 480;

/// One unpatched input on its way to rest, or one cable's share of an
/// input that keeps others.
#[derive(Clone, Copy, Default)]
struct Release {
    module: usize,
    port: usize,
    from: f32,
    to: f32,
    /// The sample the release started at.
    start: u64,
    /// A removed cable's share, added to the input's remaining routes and
    /// gliding to 0, rather than the whole input.
    share: bool,
    /// A share's cable: its source module, in the graph's order, and port
    /// (`NO_SOURCE` once that module is gone).
    source: (usize, usize),
}

/// The source of a share whose module is gone.
const NO_SOURCE: (usize, usize) = (usize::MAX, usize::MAX);

impl Release {
    /// The input's value at `sample` of a release `frames` long: one step of
    /// the glide already taken at its first sample, `to` from its last on.
    #[inline]
    fn value(&self, sample: u64, frames: u64) -> f32 {
        let taken = (sample.saturating_sub(self.start) + 1).min(frames);
        self.from + (self.to - self.from) * (taken as f32 / frames as f32)
    }

    /// What the input last heard of the release by sample `now`: where it
    /// starts, if it has not played yet.
    #[inline]
    fn heard(&self, now: u64, frames: u64) -> f32 {
        if now > self.start {
            self.value(now - 1, frames)
        } else {
            self.from
        }
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

    /// Starts `release`, or reports there is no room.
    fn begin(&mut self, release: Release) -> bool {
        if self.len == MAX_RELEASES || !release.from.is_finite() || !release.to.is_finite() {
            return false;
        }
        self.slots[self.len] = release;
        self.len += 1;
        true
    }

    /// Forgets the release of the whole of `port` of `module`, reporting
    /// whether there was one. The shares of cables it lost stay.
    pub(super) fn cancel(&mut self, module: usize, port: usize) -> bool {
        let found = (0..self.len).find(|&i| {
            let release = self.slots[i];
            release.module == module && release.port == port && !release.share
        });
        found.inspect(|&i| self.swap_remove(i)).is_some()
    }

    /// Forgets every release of `port` of `module`, returning what its
    /// input last heard of them by sample `now`.
    pub(super) fn cancel_all(&mut self, module: usize, port: usize, now: u64) -> f32 {
        let mut heard = 0.0;
        let mut i = 0;
        while i < self.len {
            let release = self.slots[i];
            if release.module == module && release.port == port {
                heard += release.heard(now, self.frames);
                self.swap_remove(i);
            } else {
                i += 1;
            }
        }
        heard
    }

    /// Moves every release to its module's index in a new module order,
    /// dropping those of modules `new_of` does not carry over, and each
    /// share's source to where `source_of` puts it.
    pub(super) fn remap(
        &mut self,
        new_of: impl Fn(usize) -> Option<usize>,
        source_of: impl Fn(usize) -> Option<usize>,
    ) {
        let mut i = 0;
        while i < self.len {
            match new_of(self.slots[i].module) {
                Some(new) => {
                    let release = &mut self.slots[i];
                    release.module = new;
                    let (from, port) = release.source;
                    let from = (from != usize::MAX).then(|| source_of(from)).flatten();
                    release.source = from.map_or(NO_SOURCE, |from| (from, port));
                    i += 1;
                }
                None => self.swap_remove(i),
            }
        }
    }

    /// Forgets one share of `port` of `module` whose cable comes from
    /// `source`.
    fn cancel_share(&mut self, module: usize, port: usize, source: (usize, usize)) {
        let found = (0..self.len).find(|&i| {
            let release = self.slots[i];
            release.share
                && release.module == module
                && release.port == port
                && release.source == source
        });
        if let Some(i) = found {
            self.swap_remove(i);
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
/// routes into them, every module's last outputs as of then (indexed as
/// those routes are), and, when those routes index another module order
/// than the graph's now, that order's module map, to match sources by id.
#[derive(Clone, Copy)]
pub(super) struct Previous<'a> {
    pub(super) ports: &'a [usize],
    pub(super) routes: &'a [CompiledRoute],
    pub(super) carry: &'a [Vec<f32>],
    pub(super) modules: Option<&'a IndexMap<String, ModuleInstance>>,
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
    /// where `previous` left it plus `shares`, what it last heard of the
    /// cables it lost before; or reports that it lets go at once (its
    /// module says so, the value is unknown, or there is no room).
    pub(super) fn release_input(
        &mut self,
        mi: usize,
        port: usize,
        previous: Previous,
        shares: f32,
    ) -> bool {
        let Some((_, inst)) = self.modules.get_index(mi) else {
            return false;
        };
        let (Some(from), Some(to)) = (previous.held(port), inst.module().input_rest(port)) else {
            return false;
        };
        self.releases.begin(Release {
            module: mi,
            port,
            from: from + shares,
            to,
            start: self.current_sample,
            share: false,
            source: NO_SOURCE,
        })
    }

    /// Glides out each cable input `port` of module `mi` had in `previous`
    /// and has no longer, the port still connected: a share of what the
    /// cable last carried, gliding to 0 on top of the routes that remain.
    /// A cable patched in again as its share glides is the routes' at once,
    /// as a port patched again as it releases is. A cable is its source
    /// module (by id, or by index when `previous` is in the graph's own
    /// order) and port, and one made twice counts twice. A share
    /// past the slots, or of a cable whose value is unknown, steps as
    /// before.
    pub(super) fn release_routes(&mut self, mi: usize, port: usize, previous: Previous) {
        let now: &[CompiledRoute] = self.compiled_routes.get(mi).map_or(&[], Vec::as_slice);
        let into = |route: &&CompiledRoute| route.to_port == port;
        // Where each route's source is now, if it still runs.
        let source_now = |old: &CompiledRoute| match previous.modules {
            None => Some(old.from_module),
            Some(modules) => {
                id_at(modules, old.from_module).and_then(|id| self.modules.get_index_of(id))
            }
        };
        let same = |old: &CompiledRoute, new: &CompiledRoute| {
            old.from_port == new.from_port && source_now(old) == Some(new.from_module)
        };
        // The same cable among routes of one generation.
        let twin = |a: &CompiledRoute, b: &&CompiledRoute| {
            a.from_module == b.from_module && a.from_port == b.from_port
        };
        let (then, now) = (previous.routes.iter().filter(into), now.iter().filter(into));
        // The same cables in the same order, as most edits leave a port:
        // none lost, none patched in again.
        let mut pairs = then.clone().zip(now.clone());
        if then.clone().count() == now.clone().count() && pairs.all(|(old, new)| same(old, new)) {
            return;
        }
        // A cable patched in again cuts its share short.
        let gliding = !self.releases.is_empty();
        for (k, new) in now.clone().enumerate().filter(|_| gliding) {
            let rank = now.clone().take(k).filter(|r| twin(new, r)).count();
            if rank >= then.clone().filter(|old| same(old, new)).count() {
                let source = (new.from_module, new.from_port);
                self.releases.cancel_share(mi, port, source);
            }
        }
        for (k, old) in then.clone().enumerate() {
            let rank = then.clone().take(k).filter(|r| twin(old, r)).count();
            if rank < now.clone().filter(|new| same(old, new)).count() {
                continue;
            }
            let carried = previous
                .carry
                .get(old.from_module)
                .and_then(|c| c.get(old.from_port));
            let Some(&from) = carried.filter(|&&from| from != 0.0) else {
                continue;
            };
            self.releases.begin(Release {
                module: mi,
                port,
                from,
                to: 0.0,
                start: self.current_sample,
                share: true,
                source: source_now(old).map_or(NO_SOURCE, |m| (m, old.from_port)),
            });
        }
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
            if release.share {
                if let Some((_, inst)) = self.modules.get_index_mut(module) {
                    let input = inst.module_mut().input_block_mut(release.port);
                    for (k, x) in input[..frames].iter_mut().enumerate() {
                        *x += release.value(first + k as u64, length);
                    }
                }
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
    /// block: cleared and declared unconnected, as an unpatched input is. A
    /// share that has played out just ends.
    pub(super) fn finish_releases(&mut self) {
        let mut i = 0;
        while i < self.releases.len {
            let release = self.releases.slots[i];
            if self.current_sample < release.start + self.releases.frames {
                i += 1;
                continue;
            }
            if release.share {
                self.releases.swap_remove(i);
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

/// The id of the module at `index` of `modules`.
pub(super) fn id_at(modules: &IndexMap<String, ModuleInstance>, index: usize) -> Option<&str> {
    modules.get_index(index).map(|(id, _)| id.as_str())
}

#[cfg(test)]
mod tests;
