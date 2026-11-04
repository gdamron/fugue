//! The repetition loop: the random draws of the last `loop_length` steps,
//! replayed or replaced as `repetition` decides.
//!
//! The loop keeps raw draws, not decisions, so a locked loop still answers
//! `bias`, `spread` and the scale as they move: the same chances, read
//! through the controls as they are now.

use rand::rngs::StdRng;
use rand::RngExt;

use super::controls::MAX_LOOP;

/// One step's draws, each uniform in [0, 1).
#[derive(Clone, Copy, Default)]
pub(super) struct Draw {
    /// The rhythm's coin tosses: `gate_1`'s, then `gate_2`'s.
    pub(super) coins: [f32; 2],
    /// How far into the step each gate is delayed, as a share of `jitter`.
    pub(super) jitter: [f32; 2],
    /// Each melody channel's value.
    pub(super) values: [f32; 3],
}

impl Draw {
    fn fresh(rng: &mut StdRng) -> Self {
        Self {
            coins: [rng.random(), rng.random()],
            jitter: [rng.random(), rng.random()],
            values: [rng.random(), rng.random(), rng.random()],
        }
    }
}

pub(super) struct Memory {
    steps: [Draw; MAX_LOOP],
}

impl Memory {
    /// A loop filled from `rng`, so even a loop locked from the first step
    /// plays a phrase.
    pub(super) fn new(rng: &mut StdRng) -> Self {
        let mut memory = Self {
            steps: [Draw::default(); MAX_LOOP],
        };
        memory.refill(rng);
        memory
    }

    /// Forgets the loop, filling it with new draws from `rng`.
    pub(super) fn refill(&mut self, rng: &mut StdRng) {
        for step in &mut self.steps {
            *step = Draw::fresh(rng);
        }
    }

    /// The draws for loop position `position`: replayed with the chance
    /// `repetition` sets, otherwise drawn anew and remembered there. The
    /// same seed and the same control changes always play the same.
    pub(super) fn next(&mut self, rng: &mut StdRng, position: usize, repetition: f32) -> Draw {
        let position = position % MAX_LOOP;
        // A gentle curve: the middle of the range keeps most of the loop
        // and changes a few steps each pass, so a phrase mutates rather
        // than dissolving.
        let unchanged = 1.0 - (1.0 - repetition) * (1.0 - repetition);
        let decision: f32 = rng.random();
        if decision >= unchanged {
            self.steps[position] = Draw::fresh(rng);
        }
        self.steps[position]
    }
}
