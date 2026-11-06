//! The drums: each pairs an analog-style circuit model on `audio` with a
//! synthetic one on `aux`, struck by the trigger and shaping their own
//! decay, as the reference's drum engines do.

mod analog_bass;
mod analog_snare;
mod bass;
mod filters;
mod overdrive;
mod snare;
mod synthetic_bass;
mod synthetic_snare;

pub(super) use self::bass::BassDrumEngine;
pub(super) use self::snare::SnareDrumEngine;
