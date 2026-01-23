mod audio;
mod clock_signal;
mod control;
mod frequency_signal;

pub use audio::Audio;
pub use clock_signal::ClockSignal;
pub use control::Control;
pub use frequency_signal::FrequencySignal;

// Legacy type aliases for backward compatibility during migration
pub type AudioSignal = Audio;
pub type ControlSignal = Audio;
pub type GateSignal = Audio;
pub type TriggerSignal = Audio;
