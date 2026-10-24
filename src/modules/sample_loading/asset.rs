//! Asset controls: a live write to a sample module's `asset` or `asset.N`,
//! loaded on a control thread and handed to the module as a payload.
//!
//! A sample module attaches an [`AssetLoader`] to its declared surface at
//! build ([`DeclaredSurface::with_context`]), or a context of its own that
//! holds one (an instrument's zones add their loop points). Its codec loads
//! each write through it, so a write loads as the module's build loaded its
//! samples: at the engine rate, under the same build lock. A load that fails
//! refuses the write with the reason, and nothing reaches the audio thread.
//!
//! A relative path is resolved before it gets here, against the invention's
//! file (`invention::audio_assets::live`), and a write is recorded into its
//! entry's `asset` by the document (`invention::authored_document`): both
//! see only the key and the value, so neither changes with the module.
//!
//! [`DeclaredSurface::with_context`]: crate::invention::declared::DeclaredSurface::with_context

// Shared by the sample modules as they move onto declared controls; until
// then only the test sampler loads through it.
#![cfg_attr(not(test), allow(dead_code))]

use std::sync::Arc;

use super::{load_cached_sample, resolve_source, SampleData};
use crate::control_request::{PayloadCodec, PayloadContext, Prepared};
use crate::payload::Payload;
use crate::pkg::lock::BuildPolicy;
use crate::ControlValue;

/// What a sample module's asset writes load with: the engine rate and the
/// lock the module was built under (see `BuildPolicy`).
#[derive(Clone)]
pub(crate) struct AssetLoader {
    sample_rate: u32,
    policy: BuildPolicy,
}

impl AssetLoader {
    /// A loader for a module building now, at `sample_rate`, under the lock
    /// governing this thread.
    pub(crate) fn current(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            policy: BuildPolicy::current(),
        }
    }

    /// Resolves `asset` (a package ref, path or https URL) and loads it at
    /// the engine rate through the sample cache, which keeps the buffer
    /// (see [`SampleData`]). Control thread: may read, decode and resample.
    pub(crate) fn load(&self, asset: &str) -> Result<Arc<SampleData>, String> {
        (self.policy).run(|| load_cached_sample(&resolve_source(asset)?, self.sample_rate))
    }
}

/// The codec of a control whose payload is one sample (a player's `asset`,
/// a kit slot's `asset.N`), for a module that attached an [`AssetLoader`].
/// The payload is the cache's own `Arc<SampleData>`, kept by the module as a
/// `Shared<SampleData>`; the control reads back the value as written.
pub(crate) const SAMPLE_ASSET: PayloadCodec = PayloadCodec {
    prepare: prepare_sample,
};

fn prepare_sample(context: &PayloadContext<'_>, value: &ControlValue) -> Prepared {
    let sample = context.module::<AssetLoader>()?.load(value.as_string()?)?;
    Ok((Payload::from_arc(sample), value.clone()))
}
