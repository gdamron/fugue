//! Grace-note realization: a small integer state machine that plays a step's
//! grace chain as short attacks on the sequencer's mono frequency/gate/velocity
//! stream, ahead of the principal note.
//!
//! The player is pure per-sample state — no allocation, no locks, no float
//! accumulation — so it can live inside a sequencer's audio-thread hot path.
//! Each grace occupies `per_grace` samples: sounding first, then a short
//! release gap so the next onset (the following grace or the principal note)
//! presents a real rising edge to downstream envelopes and voice allocators.
//! One-sample dips are deliberately avoided here for the same reason FUG-189
//! moved retriggers to explicit release gaps.

use super::step::GraceChain;

/// Smallest useful grace slot: 2 samples sounding + the 2-sample minimum gap.
pub(crate) const MIN_GRACE_SLOT: u32 = 4;

/// Default duration of a single grace note in milliseconds.
pub(crate) const DEFAULT_GRACE_DURATION_MS: f32 = 60.0;
/// Range of the `grace_duration_ms` control.
pub(crate) const MIN_GRACE_DURATION_MS: f32 = 5.0;
pub(crate) const MAX_GRACE_DURATION_MS: f32 = 200.0;
/// Default velocity scale for grace notes relative to the decorated step.
pub(crate) const DEFAULT_GRACE_VELOCITY: f32 = 0.8;

/// Samples of a grace slot spent with the gate low.
pub(crate) fn release_gap(per_grace: u32) -> u32 {
    (per_grace / 8).max(2)
}

/// Shrinks the desired per-grace duration so the whole chain fits inside
/// `window` samples (half a step, by convention), never below [`MIN_GRACE_SLOT`].
pub(crate) fn clamp_per_grace(chain: &GraceChain, desired: u32, window: u32) -> u32 {
    let count = chain.len() as u32;
    if count == 0 {
        return desired.max(MIN_GRACE_SLOT);
    }
    (window / count).min(desired).max(MIN_GRACE_SLOT)
}

/// What a grace chain contributes to the outputs for one sample.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GraceVoice {
    /// Semitone offset from the base note of the grace sounding now.
    pub offset: i8,
    /// Whether the gate is high this sample (low during the release gap).
    pub gate: bool,
    /// Velocity for the grace (decorated step's amplitude, scaled).
    pub velocity: f32,
}

/// Plays one grace chain, one sample at a time.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GracePlayer {
    chain: GraceChain,
    velocity: f32,
    per_grace: u32,
    index: usize,
    /// Samples remaining in the current grace's slot (sounding + gap).
    remaining: u32,
    active: bool,
}

impl GracePlayer {
    pub fn idle() -> Self {
        Self {
            chain: GraceChain::default(),
            velocity: 1.0,
            per_grace: 0,
            index: 0,
            remaining: 0,
            active: false,
        }
    }

    /// Total samples a chain occupies at `per_grace` samples per grace.
    pub fn chain_samples(chain: &GraceChain, per_grace: u32) -> u32 {
        chain.len() as u32 * per_grace
    }

    /// Begins playing `chain` immediately, replacing any playback in flight.
    pub fn start(&mut self, chain: GraceChain, velocity: f32, per_grace: u32) {
        if chain.is_empty() {
            self.cancel();
            return;
        }
        self.chain = chain;
        self.velocity = velocity;
        self.per_grace = per_grace.max(MIN_GRACE_SLOT);
        self.index = 0;
        self.remaining = self.per_grace;
        self.active = true;
    }

    /// Stops playback immediately (the decorated step's edge arrived).
    pub fn cancel(&mut self) {
        self.active = false;
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Advances one sample, returning what should sound now; `None` once the
    /// chain has finished (or was never started).
    pub fn tick(&mut self) -> Option<GraceVoice> {
        if !self.active {
            return None;
        }
        let offset = self.chain.get(self.index)?;
        let gap = release_gap(self.per_grace);
        let voice = GraceVoice {
            offset,
            gate: self.remaining > gap,
            velocity: self.velocity,
        };
        self.remaining -= 1;
        if self.remaining == 0 {
            self.index += 1;
            if self.index >= self.chain.len() {
                self.active = false;
            } else {
                self.remaining = self.per_grace;
            }
        }
        Some(voice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plays_chain_in_order_with_release_gaps() {
        let chain = GraceChain::from_slice(&[-2, 3]).unwrap();
        let mut player = GracePlayer::idle();
        player.start(chain, 0.8, 16);

        let mut samples = Vec::new();
        while let Some(voice) = player.tick() {
            samples.push((voice.offset, voice.gate));
        }
        assert_eq!(samples.len(), 32);

        // First grace: 14 sounding, 2 gap; then the second likewise.
        assert!(samples[..14].iter().all(|&(o, g)| o == -2 && g));
        assert!(samples[14..16].iter().all(|&(o, g)| o == -2 && !g));
        assert!(samples[16..30].iter().all(|&(o, g)| o == 3 && g));
        assert!(samples[30..].iter().all(|&(o, g)| o == 3 && !g));
        assert!(!player.is_active());
        assert!(player.tick().is_none());
    }

    #[test]
    fn clamps_slot_to_window_and_floor() {
        let chain = GraceChain::from_slice(&[0, 1, 2, 3]).unwrap();
        // Window smaller than desired: shrink proportionally.
        assert_eq!(clamp_per_grace(&chain, 1000, 400), 100);
        // Desired smaller than window: keep desired.
        assert_eq!(clamp_per_grace(&chain, 50, 400), 50);
        // Never below the minimum slot.
        assert_eq!(clamp_per_grace(&chain, 1000, 4), MIN_GRACE_SLOT);
    }

    #[test]
    fn cancel_stops_playback_mid_chain() {
        let chain = GraceChain::from_slice(&[5]).unwrap();
        let mut player = GracePlayer::idle();
        player.start(chain, 1.0, 8);
        assert!(player.tick().is_some());
        player.cancel();
        assert!(player.tick().is_none());
    }
}
