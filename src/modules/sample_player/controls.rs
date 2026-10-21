//! Thread-safe controls for the SamplePlayer module.
//!
//! The audio thread never blocks here. It reads `play`, `loop` and the play
//! trigger from atomics once per block, and takes a staged sample with
//! `try_lock`, only after `swap_version` moved (the counter pattern from
//! `sample_kit`). When a contended `try_lock` fails, the swap lands on a
//! later block.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, TryLockError};

use crate::atomic::AtomicF32;
use crate::modules::sample_loading::{load_cached_sample, resolve_source, SampleData};
use crate::{ControlMeta, ControlSurface, ControlValue};

#[derive(Clone)]
pub struct SamplePlayerControls {
    pub(crate) shared: Arc<SamplePlayerShared>,
    pitch_ratio: AtomicF32,
    time_ratio: AtomicF32,
    sample_rate: u32,
    /// Fixed at build time from the `mode` config key. In elastic mode
    /// `pitch_ratio` is pitch-only and `time_ratio` speed-only; in classic
    /// mode `pitch_ratio` keeps its coupled speed+pitch meaning.
    elastic: bool,
}

pub(crate) struct SamplePlayerShared {
    /// Bit 0 is `play`; the bits above count `play = true` writes, so every
    /// retrigger is seen even if `play` was already on. The audio thread's
    /// end-of-sample write is a compare-and-swap against the value it read
    /// at the top of the block, so a retrigger or stop written meanwhile
    /// wins over it.
    transport: AtomicU64,
    loop_enabled: AtomicBool,
    /// Authored asset ref, read and written by control threads only.
    asset: Mutex<String>,
    /// Bumped after a sample is staged in `swap`.
    swap_version: AtomicU64,
    swap: Mutex<SampleSwap>,
}

#[derive(Default)]
pub(crate) struct SampleSwap {
    pending: Option<Arc<SampleData>>,
    /// The sample the audio thread swapped out. The next control-thread
    /// swap drops it, so the audio thread never frees a sample. Until then
    /// it stays in memory (usually shared with the sample cache anyway).
    retired: Option<Arc<SampleData>>,
}

/// The `play` bit of a transport value.
pub(crate) fn transport_play(transport: u64) -> bool {
    transport & 1 == 1
}

/// The play trigger count of a transport value.
pub(crate) fn transport_trigger(transport: u64) -> u64 {
    transport >> 1
}

impl SamplePlayerControls {
    pub fn new(
        sample_rate: u32,
        asset: Option<&str>,
        play: Option<bool>,
        loop_enabled: Option<bool>,
    ) -> Result<Self, String> {
        Self::with_mode(sample_rate, asset, play, loop_enabled, false)
    }

    pub fn with_mode(
        sample_rate: u32,
        asset: Option<&str>,
        play: Option<bool>,
        loop_enabled: Option<bool>,
        elastic: bool,
    ) -> Result<Self, String> {
        let controls = Self {
            shared: Arc::new(SamplePlayerShared {
                transport: AtomicU64::new(0),
                loop_enabled: AtomicBool::new(false),
                asset: Mutex::new(String::new()),
                swap_version: AtomicU64::new(0),
                swap: Mutex::new(SampleSwap::default()),
            }),
            pitch_ratio: AtomicF32::new(1.0),
            time_ratio: AtomicF32::new(1.0),
            sample_rate,
            elastic,
        };

        if let Some(asset) = asset {
            if !asset.is_empty() {
                controls.set_asset(asset)?;
            }
        }

        if let Some(play) = play {
            controls.set_play(play);
        }

        if let Some(loop_enabled) = loop_enabled {
            controls.set_loop_enabled(loop_enabled);
        }

        Ok(controls)
    }

    pub fn asset(&self) -> String {
        self.shared.asset.lock().unwrap().clone()
    }

    pub fn set_asset(&self, asset: &str) -> Result<(), String> {
        let target = resolve_source(asset)?;
        let sample = load_cached_sample(&target, self.sample_rate)?;
        if self.elastic {
            // Elastic playback needs the per-asset analysis; computing it
            // here keeps the allocation off the audio thread (the reader
            // finds it on the SampleData via the lock-free accessor).
            sample.elastic_analysis();
        }
        // The authored ref stays the control value, so a saved document keeps
        // the portable form rather than this machine's cache path.
        let mut asset_ref = self.shared.asset.lock().unwrap();
        let (replaced, retired) = {
            let mut swap = self.shared.swap.lock().unwrap();
            (swap.pending.replace(sample), swap.retired.take())
        };
        *asset_ref = asset.to_string();
        self.shared.swap_version.fetch_add(1, Ordering::Release);
        drop(asset_ref);
        // Dropped here, off the audio thread and outside the swap lock.
        drop((replaced, retired));
        Ok(())
    }

    pub fn play(&self) -> bool {
        transport_play(self.transport())
    }

    pub fn set_play(&self, play: bool) {
        if play {
            let _ = self.shared.transport.fetch_update(
                Ordering::AcqRel,
                Ordering::Acquire,
                |transport| Some((transport | 1).wrapping_add(2)),
            );
        } else {
            self.shared.transport.fetch_and(!1, Ordering::AcqRel);
        }
    }

    pub fn loop_enabled(&self) -> bool {
        self.shared.loop_enabled.load(Ordering::Acquire)
    }

    pub fn set_loop_enabled(&self, loop_enabled: bool) {
        self.shared
            .loop_enabled
            .store(loop_enabled, Ordering::Release);
    }

    /// Audio thread: `play` and the play trigger, read once per block.
    pub(crate) fn transport(&self) -> u64 {
        self.shared.transport.load(Ordering::Acquire)
    }

    /// Audio thread: turns `play` off when a one-shot sample ends, unless a
    /// control thread wrote `play` since `observed` was read. Returns whether
    /// it did.
    pub(crate) fn end_play(&self, observed: u64) -> bool {
        self.shared
            .transport
            .compare_exchange(observed, observed & !1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Audio thread: the swap counter, checked once per block.
    pub(crate) fn swap_version(&self) -> u64 {
        self.shared.swap_version.load(Ordering::Acquire)
    }

    /// Audio thread: exchanges `current` for the staged sample, if one is
    /// staged and the lock is free. The replaced sample is parked for a
    /// control thread to drop. Returns `None` when the lock was busy, so the
    /// caller retries on a later block; `Some(swapped)` otherwise.
    ///
    /// On a swap, `transport` is read again under the lock: `play` as it
    /// stood once the swapped-in sample was staged (a stop written before it
    /// included), with no later sample staged before the read.
    pub(crate) fn try_swap(
        &self,
        current: &mut Option<Arc<SampleData>>,
        transport: &mut u64,
    ) -> Option<bool> {
        let mut swap = match self.shared.swap.try_lock() {
            Ok(swap) => swap,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return None,
        };
        // Nothing staged: a swap that took the sample staged before its
        // version moved leaves the version still to catch up with, and the
        // replaced sample parked. That is no reason to retry.
        if swap.pending.is_none() {
            return Some(false);
        }
        // `retired` is empty whenever `pending` is full: the staging write
        // empties it, and only one swap follows each staging. Should that
        // ever not hold, wait for a control thread rather than free here.
        if swap.retired.is_some() {
            return None;
        }
        let Some(sample) = swap.pending.take() else {
            return Some(false);
        };
        swap.retired = current.replace(sample);
        *transport = self.transport();
        Some(true)
    }

    /// Holds the swap lock, as a control thread mid-`set_asset` would.
    #[cfg(test)]
    pub(crate) fn hold_swap_lock(&self) -> std::sync::MutexGuard<'_, SampleSwap> {
        self.shared.swap.lock().unwrap()
    }

    /// The staged sample not yet taken by the audio thread.
    #[cfg(test)]
    pub(crate) fn pending_sample(&self) -> Option<Arc<SampleData>> {
        self.shared.swap.lock().unwrap().pending.clone()
    }

    pub fn pitch_ratio(&self) -> f32 {
        self.pitch_ratio.load()
    }

    pub fn set_pitch_ratio(&self, pitch_ratio: f32) {
        // Clamp to a small positive floor so the read head always advances
        // forward; a zero or negative ratio would stall or reverse playback.
        self.pitch_ratio.store(pitch_ratio.max(1e-4));
    }

    pub fn time_ratio(&self) -> f32 {
        self.time_ratio.load()
    }

    pub fn set_time_ratio(&self, time_ratio: f32) {
        self.time_ratio.store(time_ratio.max(1e-4));
    }

    pub(crate) fn is_elastic(&self) -> bool {
        self.elastic
    }

    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

impl ControlSurface for SamplePlayerControls {
    fn controls(&self) -> Vec<ControlMeta> {
        let pitch_description = if self.elastic {
            "Pitch ratio (1.0 = native, 2.0 = up an octave) without changing speed"
        } else {
            "Playback speed / pitch ratio (1.0 = native, 2.0 = up an octave)"
        };
        let mut controls = vec![
            ControlMeta::string(
                "asset",
                "Audio sample path, https URL, or package ref like \
                 'fugue.drums.808@1.2.0:kick/long.wav' (WAV or FLAC)",
            )
            .with_default(self.asset()),
            ControlMeta::boolean("play", "Start or stop sample playback", self.play()),
            ControlMeta::boolean("loop", "Loop playback when enabled", self.loop_enabled()),
            ControlMeta::number("pitch_ratio", pitch_description)
                .with_range(0.25, 4.0)
                .with_default(self.pitch_ratio()),
        ];
        if self.elastic {
            controls.push(
                ControlMeta::number(
                    "time_ratio",
                    "Playback speed ratio (1.0 = native, 2.0 = double speed) \
                     without changing pitch",
                )
                .with_range(0.25, 4.0)
                .with_default(self.time_ratio()),
            );
        }
        controls
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "asset" => Ok(self.asset().into()),
            "play" => Ok(self.play().into()),
            "loop" => Ok(self.loop_enabled().into()),
            "pitch_ratio" => Ok(self.pitch_ratio().into()),
            "time_ratio" if self.elastic => Ok(self.time_ratio().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "asset" => self.set_asset(value.as_string()?)?,
            "play" => self.set_play(value.as_bool()?),
            "loop" => self.set_loop_enabled(value.as_bool()?),
            "pitch_ratio" => self.set_pitch_ratio(value.as_number()?),
            "time_ratio" if self.elastic => self.set_time_ratio(value.as_number()?),
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }
}
