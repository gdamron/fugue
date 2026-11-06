//! The noise engines: noise shaped by filters, unpitched, and left to the
//! low-pass gate to shape over time.

mod clocked;
mod diffuser;
mod filtered;
mod particle;
mod particle_noise;

pub(super) use self::filtered::FilteredNoiseEngine;
pub(super) use self::particle_noise::ParticleNoiseEngine;
