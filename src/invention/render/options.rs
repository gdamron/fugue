//! Options that make an offline render repeatable.

use super::RenderEngine;

impl RenderEngine {
    /// Sets the invention seed the next load (and [`Self::reset`]) builds
    /// under; `None`, the default, leaves unseeded random sources to
    /// entropy, so the piece plays differently each time.
    ///
    /// With a seed, every melody, chance and noise module the invention
    /// leaves unseeded, including those inside developments, plays a stream
    /// derived from `seed` and its module id: rendering the same invention
    /// twice gives the same samples, and another seed gives another
    /// performance. A module with a seed of its own keeps it, and the
    /// document is left as written (its modules still show no seed).
    /// Modules an edit builds after the load are not covered. A render
    /// repeats only where nothing else varies: `code` and `agent` modules
    /// run on wall-clock time, so an invention using them still varies.
    pub fn set_seed(&mut self, seed: Option<u64>) {
        self.seed = seed;
    }

    /// The invention seed loads build under, if any (see [`Self::set_seed`]).
    pub fn seed(&self) -> Option<u64> {
        self.seed
    }

    /// Stubs the backends of the `agent` modules the next load starts: a
    /// triggered agent contacts no network service and starts no local
    /// process, and reports the request as refused in its `last_error`.
    /// For test renders; agents call their configured backends otherwise.
    pub fn stub_agent_backends(&mut self, stubbed: bool) {
        self.agents.set_stubbed(stubbed);
    }
}
