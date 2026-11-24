//! Acceptance tests for the output meters' RMS, written from the contract
//! alone.
//!
//! Contract: RMS is measured on the output as the DAC receives it, each
//! sample through the device clamp (-1 to just below 1.0, non-finite silenced
//! to 0), so it matches the in-chat player's sqrt(sum of squares / frames)
//! over the samples `RenderEngine::render_interleaved` writes, within 0.1 dB.
//! Peak stays the pre-clamp reading. This holds for the live meter
//! (`RunningInvention::master_meter`, the source of `MeterReading`) and the
//! offline `RenderEngine::take_levels`.

mod live;
mod offline;
mod support;
