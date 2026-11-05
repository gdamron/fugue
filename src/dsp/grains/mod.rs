//! Granular building blocks: a circular stereo recording; a pool of
//! grains, a time-stretcher and a looping delay that read it; a pitch
//! shifter and a smear for what they play; and the edge and clock readers
//! that start grains in time with a patch.
//!
//! Ported in part from Mutable Instruments' Clouds (MIT, Copyright 2014
//! Emilie Gillet); each file says what it took from where. See
//! THIRD_PARTY_NOTICES.md.

mod clock;
mod cloud;
mod diffuser;
mod looper;
mod pitch_shift;
mod recording;
mod stretch;

pub use clock::{ClockDivider, RisingEdge};
pub use cloud::{share_per_frame, GrainCloud, GrainShape, Onsets, MAX_GRAINS};
pub use diffuser::Diffuser;
pub use looper::{LoopShape, Looper, MIDDLE_INERTIA};
pub use pitch_shift::PitchShifter;
pub use recording::Recording;
pub use stretch::{StretchShape, Stretcher};
