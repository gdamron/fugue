//! Modular synthesis components.
//!
//! This module contains all the building blocks for creating modular synthesis setups:
//! - [`Clock`] / [`ClockControls`] - Timing and tempo control
//! - [`Oscillator`] / [`OscillatorControls`] / [`OscillatorType`] - Waveform generation
//! - [`Lfo`] / [`LfoControls`] - Low frequency oscillator for modulation
//! - [`Filter`] / [`FilterControls`] / [`FilterType`] - Resonant filter for subtractive synthesis
//! - [`Mixer`] / [`MixerControls`] - Multi-channel audio mixer
//! - [`MelodyGenerator`] / [`MelodyControls`] - Algorithmic melody generation
//! - [`StepSequencer`] / [`Step`] - Deterministic step sequencer
//! - [`Adsr`] / [`AdsrControls`] - Envelope generator
//! - [`Vca`] / [`VcaControls`] - Voltage controlled amplifier
//! - [`DacModule`] - Audio output sink module
//! - [`AudioFileSink`] - Audio file recording sink module
//! - [`AudioDriver`] / [`AudioBackend`] - Audio output backends
//! - [`SampleSlicer`] - Indexed slice playback for loops and breakbeats
//!
//! Each module also provides a factory for self-contained construction:
//! - [`ClockFactory`], [`OscillatorFactory`], [`LfoFactory`], [`FilterFactory`], [`MixerFactory`], [`AdsrFactory`], [`VcaFactory`], [`MelodyFactory`], [`StepSequencerFactory`], [`DacFactory`]

pub mod adsr;
pub mod agent;
pub mod audio_file_sink;
pub mod cell_sequencer;
pub mod clock;
pub mod code;
pub mod control_scheduler;
pub mod dac;
pub mod divisi;
pub mod filter;
pub mod lfo;
pub mod melody;
pub mod mixer;
pub mod oscillator;
pub mod reverb;
#[cfg(not(target_arch = "wasm32"))]
pub mod rtmp_sink;
pub mod sample_kit;
pub(crate) mod sample_loading;
pub mod sample_player;
pub mod sample_slicer;
pub mod step_sequencer;
pub mod sustain;
pub mod vca;
#[cfg(not(target_arch = "wasm32"))]
pub mod youtube_sink;

// Re-export module types
pub use adsr::{Adsr, AdsrControls};
pub use agent::AgentControls;
pub use audio_file_sink::{
    AudioFileSink, AudioFileSinkFactory, AudioFileSinkHandle, AudioFileSinkStats,
};
pub use cell_sequencer::{CellSequencer, CellSequencerControls};
pub use clock::{Clock, ClockControls};
pub use code::CodeControls;
pub use control_scheduler::{
    ControlScheduler, ControlSchedulerControls, ScheduleEntry, ScheduleValue,
};

// Deprecated type alias for backward compatibility
#[allow(deprecated)]
pub use dac::{
    default_sample_rate, AudioBackend, AudioDiagnostics, AudioDiagnosticsSnapshot, AudioDriver,
    DacModule,
};
pub use filter::{Filter, FilterControls, FilterType};
pub use lfo::{Lfo, LfoControls};
pub use melody::{MelodyControls, MelodyGenerator};

pub use mixer::{Mixer, MixerControls};
pub use oscillator::{Oscillator, OscillatorControls, OscillatorType};
pub use reverb::{Reverb, ReverbControls};
#[cfg(not(target_arch = "wasm32"))]
pub use rtmp_sink::{RtmpSink, RtmpSinkConfig, RtmpSinkHandle, RtmpSinkStats};
pub use sample_kit::{SampleKit, SampleKitControls};
pub use sample_player::{SamplePlayer, SamplePlayerControls};
pub use sample_slicer::SampleSlicer;
pub use step_sequencer::{GraceChain, Step, StepSequencer, MAX_GRACE_NOTES};
pub use sustain::{Sustain, SustainFactory};
pub use vca::{Vca, VcaControls};
#[cfg(not(target_arch = "wasm32"))]
pub use youtube_sink::{YoutubeSink, YoutubeSinkHandle, YoutubeSinkStats};

// Re-export factory types
pub use adsr::AdsrFactory;
pub use agent::AgentFactory;
pub use cell_sequencer::CellSequencerFactory;
pub use clock::ClockFactory;
pub use code::CodeFactory;
pub use control_scheduler::ControlSchedulerFactory;
pub use dac::DacFactory;
pub use divisi::{Divisi, DivisiFactory};
pub use filter::FilterFactory;
pub use lfo::LfoFactory;
pub use melody::MelodyFactory;
pub use mixer::MixerFactory;
pub use oscillator::OscillatorFactory;
pub use reverb::ReverbFactory;
#[cfg(not(target_arch = "wasm32"))]
pub use rtmp_sink::RtmpSinkFactory;
pub use sample_kit::SampleKitFactory;
pub use sample_player::SamplePlayerFactory;
pub use sample_slicer::SampleSlicerFactory;
pub use step_sequencer::StepSequencerFactory;
pub use vca::VcaFactory;
#[cfg(not(target_arch = "wasm32"))]
pub use youtube_sink::YoutubeSinkFactory;
