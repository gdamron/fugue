//! Native (non-wasm) backend for [`AudioFileSink`].
//!
//! Audio frames are handed off from the audio thread through a lock-free ring
//! and drained by a background writer thread. WAV is streamed incrementally;
//! FLAC accumulates the full stream and encodes it on finalize (the encoder is
//! batch-oriented). All file I/O happens on the writer thread, never the audio
//! thread.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use hound::{SampleFormat, WavSpec, WavWriter};

use super::{AudioFileSink, AudioFileSinkHandle};

const DEFAULT_BUFFER_FRAMES: usize = 65_536;
/// Bit depth used when encoding FLAC output. 24-bit preserves most of the f32
/// graph signal's dynamic range while keeping files lossless.
const FLAC_BITS_PER_SAMPLE: u32 = 24;

/// Shared handle type used by [`AudioFileSink`] on native targets.
pub(super) type SharedHandle = Arc<NativeAudioFileSinkShared>;

/// Parses the native sink configuration and constructs the module.
pub(super) fn build(
    config: &serde_json::Value,
    sample_rate: u32,
) -> Result<(AudioFileSink, AudioFileSinkHandle), Box<dyn std::error::Error>> {
    let path = config
        .get("path")
        .and_then(|value| value.as_str())
        .ok_or("audio_file_sink requires config.path")?;
    let soft_clip = config
        .get("soft_clip")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    let monitor = config
        .get("monitor")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let buffer_frames = config
        .get("buffer_frames")
        .and_then(|value| value.as_u64())
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_BUFFER_FRAMES);

    let format = OutputFormat::from_path(path);
    AudioFileSink::new(
        path.into(),
        format,
        sample_rate,
        soft_clip,
        monitor,
        buffer_frames,
    )
}

/// Container format for rendered audio, chosen from the output path extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Wav,
    Flac,
}

impl OutputFormat {
    fn from_path(path: &str) -> Self {
        let ext = path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(path)
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase());
        match ext.as_deref() {
            Some("flac") => OutputFormat::Flac,
            _ => OutputFormat::Wav,
        }
    }
}

impl AudioFileSink {
    pub fn new(
        path: PathBuf,
        format: OutputFormat,
        sample_rate: u32,
        soft_clip: bool,
        monitor: bool,
        buffer_frames: usize,
    ) -> Result<(Self, AudioFileSinkHandle), Box<dyn std::error::Error>> {
        if buffer_frames == 0 {
            return Err("audio_file_sink buffer_frames must be greater than zero".into());
        }

        let file = File::create(&path)
            .map_err(|err| format!("Failed to create '{}': {}", path.display(), err))?;
        let writer = match format {
            OutputFormat::Wav => FileWriter::Wav(WavWriter::new(
                BufWriter::new(file),
                WavSpec {
                    channels: 2,
                    sample_rate,
                    bits_per_sample: 32,
                    sample_format: SampleFormat::Float,
                },
            )?),
            OutputFormat::Flac => FileWriter::Flac {
                file: BufWriter::new(file),
                sample_rate,
                samples: Vec::new(),
            },
        };

        let shared = Arc::new(NativeAudioFileSinkShared {
            ring: AudioFrameRing::new(buffer_frames),
            stopping: AtomicBool::new(false),
            frames_written: AtomicUsize::new(0),
            join_handle: Mutex::new(None),
        });

        let worker_shared = shared.clone();
        let join_handle = thread::spawn(move || write_worker(worker_shared, writer));
        *shared.join_handle.lock().unwrap() = Some(join_handle);

        Ok(Self::from_shared(shared, soft_clip, monitor))
    }
}

/// Cross-platform state shared between the audio thread, the writer thread, and
/// the host handle.
pub(super) struct NativeAudioFileSinkShared {
    ring: AudioFrameRing,
    stopping: AtomicBool,
    frames_written: AtomicUsize,
    join_handle: Mutex<Option<JoinHandle<()>>>,
}

impl NativeAudioFileSinkShared {
    #[inline]
    pub(super) fn push(&self, left: f32, right: f32) {
        self.ring.push(left, right);
    }

    #[inline]
    pub(super) fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    pub(super) fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }

    /// Signals the writer thread to finish and blocks until it has drained the
    /// ring and finalized the file.
    pub(super) fn finish(&self) {
        self.stop();
        if let Some(join_handle) = self.join_handle.lock().unwrap().take() {
            let _ = join_handle.join();
        }
    }

    pub(super) fn frames_written(&self) -> usize {
        self.frames_written.load(Ordering::Acquire)
    }

    pub(super) fn frames_dropped(&self) -> usize {
        self.ring.dropped.load(Ordering::Acquire)
    }
}

/// Background-thread writer for the selected output format.
enum FileWriter {
    Wav(WavWriter<BufWriter<File>>),
    Flac {
        file: BufWriter<File>,
        sample_rate: u32,
        samples: Vec<i32>,
    },
}

impl FileWriter {
    fn write_frame(&mut self, left: f32, right: f32) -> Result<(), ()> {
        match self {
            FileWriter::Wav(writer) => {
                writer.write_sample(left).map_err(|_| ())?;
                writer.write_sample(right).map_err(|_| ())?;
                Ok(())
            }
            FileWriter::Flac { samples, .. } => {
                samples.push(float_to_i24(left));
                samples.push(float_to_i24(right));
                Ok(())
            }
        }
    }

    fn finalize(self) -> Result<(), String> {
        match self {
            FileWriter::Wav(writer) => writer.finalize().map_err(|err| err.to_string()),
            FileWriter::Flac {
                mut file,
                sample_rate,
                samples,
            } => encode_flac(&mut file, &samples, sample_rate),
        }
    }
}

/// Converts a normalized f32 sample to a 24-bit signed integer for FLAC.
#[inline]
fn float_to_i24(sample: f32) -> i32 {
    const SCALE: f32 = ((1_i32 << (FLAC_BITS_PER_SAMPLE - 1)) - 1) as f32;
    (sample.clamp(-1.0, 1.0) * SCALE).round() as i32
}

/// Encodes interleaved stereo 24-bit samples to FLAC and writes them to `file`.
fn encode_flac(file: &mut BufWriter<File>, samples: &[i32], sample_rate: u32) -> Result<(), String> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;

    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|(_, err)| format!("FLAC encoder config error: {:?}", err))?;
    let source = flacenc::source::MemSource::from_samples(
        samples,
        2,
        FLAC_BITS_PER_SAMPLE as usize,
        sample_rate as usize,
    );
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|err| format!("FLAC encode failed: {:?}", err))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|err| format!("FLAC serialize failed: {:?}", err))?;
    file.write_all(sink.as_slice())
        .map_err(|err| format!("Failed to write FLAC: {}", err))?;
    file.flush()
        .map_err(|err| format!("Failed to flush FLAC: {}", err))?;
    Ok(())
}

/// Lock-free single-producer single-consumer ring of packed stereo frames.
struct AudioFrameRing {
    slots: Box<[AtomicU64]>,
    read_index: AtomicUsize,
    write_index: AtomicUsize,
    capacity: usize,
    dropped: AtomicUsize,
}

impl AudioFrameRing {
    fn new(capacity: usize) -> Self {
        let slots = (0..capacity)
            .map(|_| AtomicU64::new(0))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            slots,
            read_index: AtomicUsize::new(0),
            write_index: AtomicUsize::new(0),
            capacity,
            dropped: AtomicUsize::new(0),
        }
    }

    #[inline]
    fn push(&self, left: f32, right: f32) {
        let write = self.write_index.load(Ordering::Relaxed);
        let next = self.advance(write);
        if next == self.read_index.load(Ordering::Acquire) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }

        self.slots[write].store(pack_frame(left, right), Ordering::Relaxed);
        self.write_index.store(next, Ordering::Release);
    }

    fn pop(&self) -> Option<(f32, f32)> {
        let read = self.read_index.load(Ordering::Relaxed);
        if read == self.write_index.load(Ordering::Acquire) {
            return None;
        }

        let frame = unpack_frame(self.slots[read].load(Ordering::Relaxed));
        self.read_index.store(self.advance(read), Ordering::Release);
        Some(frame)
    }

    fn is_empty(&self) -> bool {
        self.read_index.load(Ordering::Acquire) == self.write_index.load(Ordering::Acquire)
    }

    #[inline]
    fn advance(&self, index: usize) -> usize {
        let next = index + 1;
        if next == self.capacity {
            0
        } else {
            next
        }
    }
}

fn write_worker(shared: Arc<NativeAudioFileSinkShared>, mut writer: FileWriter) {
    loop {
        let mut wrote = false;
        while let Some((left, right)) = shared.ring.pop() {
            if writer.write_frame(left, right).is_err() {
                shared.stopping.store(true, Ordering::Release);
                break;
            }
            shared.frames_written.fetch_add(1, Ordering::Relaxed);
            wrote = true;
        }

        if shared.stopping.load(Ordering::Acquire) && shared.ring.is_empty() {
            break;
        }

        if !wrote {
            thread::sleep(Duration::from_millis(1));
        }
    }

    let _ = writer.finalize();
}

#[inline]
fn pack_frame(left: f32, right: f32) -> u64 {
    ((left.to_bits() as u64) << 32) | right.to_bits() as u64
}

#[inline]
fn unpack_frame(frame: u64) -> (f32, f32) {
    (
        f32::from_bits((frame >> 32) as u32),
        f32::from_bits(frame as u32),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Module;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_wav_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("fugue-{}-{}.wav", name, nanos))
    }

    fn temp_flac_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("fugue-{}-{}.flac", name, nanos))
    }

    fn read_wav(path: &PathBuf) -> (hound::WavSpec, Vec<f32>) {
        let mut reader = hound::WavReader::open(path).unwrap();
        let spec = reader.spec();
        let samples = reader
            .samples::<f32>()
            .map(|sample| sample.unwrap())
            .collect();
        (spec, samples)
    }

    /// Decodes a FLAC file back into normalized f32 samples for assertions.
    fn read_flac(path: &PathBuf) -> (u32, u32, Vec<f32>) {
        let mut reader = claxon::FlacReader::open(path).unwrap();
        let info = reader.streaminfo();
        let scale = (1_i64 << (info.bits_per_sample - 1)) as f32;
        let samples = reader
            .samples()
            .map(|sample| sample.unwrap() as f32 / scale)
            .collect();
        (info.channels, info.sample_rate, samples)
    }

    #[test]
    fn writes_stereo_float_wav() {
        let path = temp_wav_path("audio-file-sink");
        let (mut sink, handle) =
            AudioFileSink::new(path.clone(), OutputFormat::Wav, 44_100, false, false, 16).unwrap();

        sink.set_input("audio_left", 0.25).unwrap();
        sink.set_input("audio_right", -0.5).unwrap();
        sink.process();
        sink.reset_inputs();
        sink.set_input("audio", 0.125).unwrap();
        sink.process();

        let stats = handle.finish();
        assert_eq!(stats.frames_written, 2);
        assert_eq!(stats.frames_dropped, 0);

        let (spec, samples) = read_wav(&path);
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate, 44_100);
        assert_eq!(spec.bits_per_sample, 32);
        assert_eq!(spec.sample_format, SampleFormat::Float);
        assert_eq!(samples, vec![0.25, -0.5, 0.125, 0.125]);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn writes_stereo_flac() {
        // FLAC requires a block size of at least 16 frames, so render a short
        // ramp rather than a couple of samples.
        const FRAMES: usize = 64;
        let path = temp_flac_path("audio-file-sink");
        let (mut sink, handle) =
            AudioFileSink::new(path.clone(), OutputFormat::Flac, 44_100, false, false, 1024)
                .unwrap();

        let mut expected = Vec::with_capacity(FRAMES * 2);
        for index in 0..FRAMES {
            let left = index as f32 / 128.0;
            let right = -(index as f32) / 128.0;
            sink.reset_inputs();
            sink.set_input("audio_left", left).unwrap();
            sink.set_input("audio_right", right).unwrap();
            sink.process();
            expected.push(left);
            expected.push(right);
        }

        let stats = handle.finish();
        assert_eq!(stats.frames_written, FRAMES);
        assert_eq!(stats.frames_dropped, 0);

        let (channels, sample_rate, samples) = read_flac(&path);
        assert_eq!(channels, 2);
        assert_eq!(sample_rate, 44_100);
        assert_eq!(samples.len(), expected.len());
        for (got, want) in samples.iter().zip(expected.iter()) {
            assert!(
                (got - want).abs() < 1e-4,
                "decoded {} expected {}",
                got,
                want
            );
        }

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn output_format_from_path_detects_flac() {
        assert_eq!(OutputFormat::from_path("/tmp/song.flac"), OutputFormat::Flac);
        assert_eq!(OutputFormat::from_path("/tmp/song.FLAC"), OutputFormat::Flac);
        assert_eq!(OutputFormat::from_path("/tmp/song.wav"), OutputFormat::Wav);
        assert_eq!(
            OutputFormat::from_path("/tmp/no_extension"),
            OutputFormat::Wav
        );
    }
}
