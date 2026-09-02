//! Audio backend that runs a graph with no output device.
//!
//! Hosts that need a *running* invention but no sound — CI, headless
//! measurement harnesses, offline tooling — start the runtime with this
//! instead of [`AudioDriver`](super::AudioDriver). Control-side work
//! (mutations, snapshots, reload) behaves normally; nothing is rendered.

use super::BlockRenderFn;

/// Backend that starts instantly and never pulls audio.
///
/// The render closure owns the graph and its command receiver, so
/// [`NullBackend`] holds it for the lifetime of the run: dropping it would
/// disconnect the command channel and silently strand every subsequent
/// mutation. That is the whole reason this stores `render` rather than
/// discarding it.
///
/// This backend never calls `render`, so no audio is produced and no time
/// passes musically. A host that needs the graph to actually advance wants a
/// clocked backend instead.
///
/// Deliberately has no `Default`: a backend reporting 0 Hz would misreport the
/// runtime's sample rate rather than fail, so the rate is always explicit.
pub struct NullBackend {
    sample_rate: u32,
    render: Option<BlockRenderFn>,
}

impl NullBackend {
    /// Creates a backend reporting `sample_rate` Hz.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            render: None,
        }
    }
}

impl super::AudioBackend for NullBackend {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(&mut self, render: BlockRenderFn) -> Result<(), Box<dyn std::error::Error>> {
        self.render = Some(render);
        Ok(())
    }

    fn stop(&mut self) {
        self.render = None;
    }
}
