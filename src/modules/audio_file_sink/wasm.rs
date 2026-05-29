//! WebAssembly backend for [`AudioFileSink`].
//!
//! There is no background thread or filesystem here: the JS host drives
//! rendering synchronously and reads the encoded bytes afterwards. Frames are
//! written directly into a pre-sized WAV byte buffer (FLAC encoding is not
//! supported on wasm).

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use super::{AudioFileSink, AudioFileSinkHandle};

const WAV_HEADER_LEN: usize = 44;
const WAV_FRAME_BYTES: usize = 8;

/// Shared handle type used by [`AudioFileSink`] on wasm targets.
pub(super) type SharedHandle = Arc<WasmAudioFileSinkShared>;

/// Parses the wasm sink configuration and constructs the module.
pub(super) fn build(
    config: &serde_json::Value,
    sample_rate: u32,
) -> Result<(AudioFileSink, AudioFileSinkHandle), Box<dyn std::error::Error>> {
    let soft_clip = config
        .get("soft_clip")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    let monitor = config
        .get("monitor")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let max_frames = max_frames(config, sample_rate)?;

    AudioFileSink::new_wasm(sample_rate, soft_clip, monitor, max_frames)
}

fn max_frames(
    config: &serde_json::Value,
    sample_rate: u32,
) -> Result<usize, Box<dyn std::error::Error>> {
    if let Some(max_frames) = config.get("max_frames").and_then(|value| value.as_u64()) {
        if max_frames > 0 {
            return Ok(max_frames as usize);
        }
        return Err("audio_file_sink max_frames must be greater than zero".into());
    }

    if let Some(max_seconds) = config.get("max_seconds").and_then(|value| value.as_f64()) {
        if max_seconds.is_finite() && max_seconds > 0.0 {
            return Ok((max_seconds * sample_rate as f64).ceil() as usize);
        }
        return Err("audio_file_sink max_seconds must be greater than zero".into());
    }

    Err("audio_file_sink on wasm requires config.max_frames or config.max_seconds".into())
}

impl AudioFileSink {
    pub fn new_wasm(
        sample_rate: u32,
        soft_clip: bool,
        monitor: bool,
        max_frames: usize,
    ) -> Result<(Self, AudioFileSinkHandle), Box<dyn std::error::Error>> {
        if max_frames == 0 {
            return Err("audio_file_sink max_frames must be greater than zero".into());
        }

        let capacity = WAV_HEADER_LEN + max_frames.saturating_mul(WAV_FRAME_BYTES);
        let mut bytes = vec![0_u8; capacity];
        write_wav_header(&mut bytes[..WAV_HEADER_LEN], sample_rate, 0);

        let shared = Arc::new(WasmAudioFileSinkShared {
            bytes: UnsafeCell::new(bytes),
            max_frames,
            sample_rate,
            stopping: AtomicBool::new(false),
            finalized: AtomicBool::new(false),
            frames_written: AtomicUsize::new(0),
            frames_dropped: AtomicUsize::new(0),
        });

        Ok(Self::from_shared(shared, soft_clip, monitor))
    }
}

pub(super) struct WasmAudioFileSinkShared {
    bytes: UnsafeCell<Vec<u8>>,
    max_frames: usize,
    sample_rate: u32,
    stopping: AtomicBool,
    finalized: AtomicBool,
    frames_written: AtomicUsize,
    frames_dropped: AtomicUsize,
}

unsafe impl Send for WasmAudioFileSinkShared {}

unsafe impl Sync for WasmAudioFileSinkShared {}

impl WasmAudioFileSinkShared {
    pub(super) fn push(&self, left: f32, right: f32) {
        if self.finalized.load(Ordering::Acquire) {
            self.frames_dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }

        let frame_index = self.frames_written.load(Ordering::Relaxed);
        if frame_index >= self.max_frames {
            self.frames_dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }

        let offset = WAV_HEADER_LEN + frame_index * WAV_FRAME_BYTES;
        // The module owns mutable graph execution; this shared buffer exists so
        // the host handle can read it after rendering is finished.
        let bytes = unsafe { &mut *self.bytes.get() };
        bytes[offset..offset + 4].copy_from_slice(&left.to_le_bytes());
        bytes[offset + 4..offset + 8].copy_from_slice(&right.to_le_bytes());
        self.frames_written
            .store(frame_index + 1, Ordering::Release);
    }

    #[inline]
    pub(super) fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    pub(super) fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }

    /// Writes the final WAV header so the byte buffer is a complete file.
    pub(super) fn finish(&self) {
        self.stop();
        let frames = self.frames_written.load(Ordering::Acquire);
        let data_bytes = frames * WAV_FRAME_BYTES;
        let bytes = unsafe { &mut *self.bytes.get() };
        write_wav_header(&mut bytes[..WAV_HEADER_LEN], self.sample_rate, data_bytes);
        self.finalized.store(true, Ordering::Release);
    }

    pub(super) fn frames_written(&self) -> usize {
        self.frames_written.load(Ordering::Acquire)
    }

    pub(super) fn frames_dropped(&self) -> usize {
        self.frames_dropped.load(Ordering::Acquire)
    }

    fn wav_bytes(&self) -> Result<Vec<u8>, String> {
        if !self.finalized.load(Ordering::Acquire) {
            return Err("audio_file_sink must be finished before reading WAV bytes".to_string());
        }

        let frames = self.frames_written.load(Ordering::Acquire);
        let len = WAV_HEADER_LEN + frames * WAV_FRAME_BYTES;
        // The WASM runtime is single-threaded here: rendering and byte export
        // are driven synchronously by the JS host.
        let bytes = unsafe { &*self.bytes.get() };
        Ok(bytes[..len].to_vec())
    }
}

impl AudioFileSinkHandle {
    pub fn wav_bytes(&self) -> Result<Vec<u8>, String> {
        self.shared.wav_bytes()
    }
}

fn write_wav_header(header: &mut [u8], sample_rate: u32, data_bytes: usize) {
    debug_assert_eq!(header.len(), WAV_HEADER_LEN);
    let riff_size = 36_u32.saturating_add(data_bytes as u32);

    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&riff_size.to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16_u32.to_le_bytes());
    header[20..22].copy_from_slice(&3_u16.to_le_bytes());
    header[22..24].copy_from_slice(&2_u16.to_le_bytes());
    header[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    header[28..32].copy_from_slice(&(sample_rate * WAV_FRAME_BYTES as u32).to_le_bytes());
    header[32..34].copy_from_slice(&(WAV_FRAME_BYTES as u16).to_le_bytes());
    header[34..36].copy_from_slice(&32_u16.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&(data_bytes as u32).to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Module;

    #[test]
    fn writes_stereo_float_wav_in_memory() {
        let (mut sink, handle) = AudioFileSink::new_wasm(44_100, false, false, 2).unwrap();

        sink.set_input("audio_left", 0.25).unwrap();
        sink.set_input("audio_right", -0.5).unwrap();
        sink.process();
        sink.reset_inputs();
        sink.set_input("audio", 0.125).unwrap();
        sink.process();

        let stats = handle.finish();
        assert_eq!(stats.frames_written, 2);
        assert_eq!(stats.frames_dropped, 0);

        let wav = handle.wav_bytes().unwrap();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 16);
        assert_eq!(f32::from_le_bytes(wav[44..48].try_into().unwrap()), 0.25);
        assert_eq!(f32::from_le_bytes(wav[48..52].try_into().unwrap()), -0.5);
        assert_eq!(f32::from_le_bytes(wav[52..56].try_into().unwrap()), 0.125);
        assert_eq!(f32::from_le_bytes(wav[56..60].try_into().unwrap()), 0.125);
    }

    #[test]
    fn wasm_drops_frames_after_capacity() {
        let (mut sink, handle) = AudioFileSink::new_wasm(44_100, false, false, 1).unwrap();

        sink.set_input("audio", 0.25).unwrap();
        sink.process();
        sink.process();

        let stats = handle.finish();
        assert_eq!(stats.frames_written, 1);
        assert_eq!(stats.frames_dropped, 1);
    }
}
