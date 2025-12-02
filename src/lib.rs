pub mod time;
pub mod synthesis;
pub mod scale;
pub mod sequencer;
pub mod audio;

pub use time::{Clock, Tempo};
pub use synthesis::{Oscillator, OscillatorType, Filter};
pub use scale::{Scale, Mode, Note};
pub use sequencer::{MelodyGenerator, MelodyParams};
pub use audio::AudioEngine;
