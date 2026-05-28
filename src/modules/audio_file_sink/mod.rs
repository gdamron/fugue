//! Audio file sink module for recording graph audio.

use std::any::Any;
#[cfg(target_arch = "wasm32")]
use std::cell::UnsafeCell;
#[cfg(not(target_arch = "wasm32"))]
use std::fs::File;
#[cfg(not(target_arch = "wasm32"))]
use std::io::BufWriter;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Mutex;
#[cfg(not(target_arch = "wasm32"))]
use std::thread::{self, JoinHandle};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use hound::{SampleFormat, WavSpec, WavWriter};

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::{Module, SinkModule, SinkOutput};

mod inputs;
mod outputs;

#[cfg(not(target_arch = "wasm32"))]
const DEFAULT_BUFFER_FRAMES: usize = 65_536;
#[cfg(target_arch = "wasm32")]
const WAV_HEADER_LEN: usize = 44;
#[cfg(target_arch = "wasm32")]
const WAV_FRAME_BYTES: usize = 8;

pub struct AudioFileSinkFactory;

impl ModuleFactory for AudioFileSinkFactory {
    fn type_id(&self) -> &'static str {
        "audio_file_sink"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        #[cfg(not(target_arch = "wasm32"))]
        let (sink, handle) = {
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

            AudioFileSink::new(path.into(), sample_rate, soft_clip, monitor, buffer_frames)?
        };

        #[cfg(target_arch = "wasm32")]
        let (sink, handle) = {
            let soft_clip = config
                .get("soft_clip")
                .and_then(|value| value.as_bool())
                .unwrap_or(true);
            let monitor = config
                .get("monitor")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let max_frames = wasm_max_frames(config, sample_rate)?;

            AudioFileSink::new_wasm(sample_rate, soft_clip, monitor, max_frames)?
        };

        Ok(ModuleBuildResult {
            module: GraphModule::Sink(Box::new(sink)),
            handles: vec![(
                "handle".to_string(),
                Arc::new(handle) as Arc<dyn Any + Send + Sync>,
            )],
            control_surface: None,
            sink: Some(()),
        })
    }

    fn is_sink(&self) -> bool {
        true
    }

    fn input_ports(&self) -> Option<&'static [&'static str]> {
        Some(&inputs::INPUTS)
    }

    fn output_ports(&self) -> Option<&'static [&'static str]> {
        Some(&outputs::OUTPUTS)
    }
}

#[cfg(target_arch = "wasm32")]
fn wasm_max_frames(
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

pub struct AudioFileSink {
    inputs: inputs::AudioFileSinkInputs,
    outputs: outputs::AudioFileSinkOutputs,
    shared: AudioFileSinkSharedHandle,
    soft_clip: bool,
    monitor: bool,
    last_processed_sample: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct AudioFileSinkStats {
    pub frames_written: usize,
    pub frames_dropped: usize,
}

#[derive(Clone)]
pub struct AudioFileSinkHandle {
    shared: AudioFileSinkSharedHandle,
}

#[cfg(not(target_arch = "wasm32"))]
type AudioFileSinkSharedHandle = Arc<NativeAudioFileSinkShared>;

#[cfg(target_arch = "wasm32")]
type AudioFileSinkSharedHandle = Arc<WasmAudioFileSinkShared>;

#[cfg(not(target_arch = "wasm32"))]
struct NativeAudioFileSinkShared {
    ring: AudioFrameRing,
    stopping: AtomicBool,
    frames_written: AtomicUsize,
    join_handle: Mutex<Option<JoinHandle<()>>>,
}

#[cfg(target_arch = "wasm32")]
struct WasmAudioFileSinkShared {
    bytes: UnsafeCell<Vec<u8>>,
    max_frames: usize,
    sample_rate: u32,
    stopping: AtomicBool,
    finalized: AtomicBool,
    frames_written: AtomicUsize,
    frames_dropped: AtomicUsize,
}

#[cfg(target_arch = "wasm32")]
unsafe impl Send for WasmAudioFileSinkShared {}

#[cfg(target_arch = "wasm32")]
unsafe impl Sync for WasmAudioFileSinkShared {}

#[cfg(not(target_arch = "wasm32"))]
struct AudioFrameRing {
    slots: Box<[AtomicU64]>,
    read_index: AtomicUsize,
    write_index: AtomicUsize,
    capacity: usize,
    dropped: AtomicUsize,
}

impl AudioFileSink {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(
        path: PathBuf,
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
        let writer = WavWriter::new(
            BufWriter::new(file),
            WavSpec {
                channels: 2,
                sample_rate,
                bits_per_sample: 32,
                sample_format: SampleFormat::Float,
            },
        )?;

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

    #[cfg(target_arch = "wasm32")]
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

    fn from_shared(
        shared: AudioFileSinkSharedHandle,
        soft_clip: bool,
        monitor: bool,
    ) -> (Self, AudioFileSinkHandle) {
        let sink = Self {
            inputs: inputs::AudioFileSinkInputs::new(),
            outputs: outputs::AudioFileSinkOutputs::new(),
            shared: shared.clone(),
            soft_clip,
            monitor,
            last_processed_sample: 0,
        };
        let handle = AudioFileSinkHandle { shared };
        (sink, handle)
    }

    pub fn with_monitor(mut self, monitor: bool) -> Self {
        self.monitor = monitor;
        self
    }

    pub fn with_soft_clip(mut self, soft_clip: bool) -> Self {
        self.soft_clip = soft_clip;
        self
    }

    #[inline]
    fn soft_clip_sample(sample: f32) -> f32 {
        const KNEE: f32 = 0.95;
        const HEADROOM: f32 = 1.0 - KNEE;

        let abs = sample.abs();
        if abs <= KNEE {
            sample
        } else {
            let excess = (abs - KNEE) / HEADROOM;
            let compressed = KNEE + HEADROOM * excess / (1.0 + excess);
            sample.signum() * compressed
        }
    }
}

impl Drop for AudioFileSink {
    fn drop(&mut self) {
        self.shared.stopping.store(true, Ordering::Release);
    }
}

impl Module for AudioFileSink {
    fn name(&self) -> &str {
        "AudioFileSink"
    }

    fn process(&mut self) -> bool {
        let (mut left, mut right) = (self.inputs.audio_left(), self.inputs.audio_right());
        if self.soft_clip {
            left = Self::soft_clip_sample(left);
            right = Self::soft_clip_sample(right);
        }
        self.outputs.set(left, right);

        if !self.shared.stopping.load(Ordering::Acquire) {
            #[cfg(not(target_arch = "wasm32"))]
            self.shared.ring.push(left, right);
            #[cfg(target_arch = "wasm32")]
            self.shared.push(left, right);
        }
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn reset_inputs(&mut self) {
        self.inputs.reset();
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    #[inline]
    fn set_input_by_index(&mut self, index: usize, value: f32) {
        self.inputs.set_by_index(index, value);
    }

    #[inline]
    fn get_output_by_index(&self, index: usize) -> f32 {
        self.outputs.get_by_index(index)
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }
}

impl SinkModule for AudioFileSink {
    fn sink_output(&self) -> SinkOutput {
        if self.monitor {
            SinkOutput::stereo(self.outputs.audio_left(), self.outputs.audio_right())
        } else {
            SinkOutput::default()
        }
    }
}

impl AudioFileSinkHandle {
    pub fn finish(&self) -> AudioFileSinkStats {
        self.shared.stopping.store(true, Ordering::Release);

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(join_handle) = self.shared.join_handle.lock().unwrap().take() {
            let _ = join_handle.join();
        }

        #[cfg(target_arch = "wasm32")]
        self.shared.finalize();

        self.stats()
    }

    pub fn stats(&self) -> AudioFileSinkStats {
        AudioFileSinkStats {
            frames_written: self.shared.frames_written.load(Ordering::Acquire),
            frames_dropped: self.frames_dropped(),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn wav_bytes(&self) -> Result<Vec<u8>, String> {
        if !self.shared.finalized.load(Ordering::Acquire) {
            return Err("audio_file_sink must be finished before reading WAV bytes".to_string());
        }

        let frames = self.shared.frames_written.load(Ordering::Acquire);
        let len = WAV_HEADER_LEN + frames * WAV_FRAME_BYTES;
        // The WASM runtime is single-threaded here: rendering and byte export
        // are driven synchronously by the JS host.
        let bytes = unsafe { &*self.shared.bytes.get() };
        Ok(bytes[..len].to_vec())
    }

    fn frames_dropped(&self) -> usize {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.shared.ring.dropped.load(Ordering::Acquire)
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.shared.frames_dropped.load(Ordering::Acquire)
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl WasmAudioFileSinkShared {
    fn push(&self, left: f32, right: f32) {
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

    fn finalize(&self) {
        let frames = self.frames_written.load(Ordering::Acquire);
        let data_bytes = frames * WAV_FRAME_BYTES;
        let bytes = unsafe { &mut *self.bytes.get() };
        write_wav_header(&mut bytes[..WAV_HEADER_LEN], self.sample_rate, data_bytes);
        self.finalized.store(true, Ordering::Release);
    }
}

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
fn write_worker(shared: Arc<NativeAudioFileSinkShared>, mut writer: WavWriter<BufWriter<File>>) {
    loop {
        let mut wrote = false;
        while let Some((left, right)) = shared.ring.pop() {
            if writer.write_sample(left).is_err() || writer.write_sample(right).is_err() {
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

#[cfg(not(target_arch = "wasm32"))]
#[inline]
fn pack_frame(left: f32, right: f32) -> u64 {
    ((left.to_bits() as u64) << 32) | right.to_bits() as u64
}

#[cfg(not(target_arch = "wasm32"))]
#[inline]
fn unpack_frame(frame: u64) -> (f32, f32) {
    (
        f32::from_bits((frame >> 32) as u32),
        f32::from_bits(frame as u32),
    )
}

#[cfg(target_arch = "wasm32")]
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
    #[cfg(not(target_arch = "wasm32"))]
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(not(target_arch = "wasm32"))]
    fn temp_wav_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("fugue-{}-{}.wav", name, nanos))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn read_wav(path: &PathBuf) -> (hound::WavSpec, Vec<f32>) {
        let mut reader = hound::WavReader::open(path).unwrap();
        let spec = reader.spec();
        let samples = reader
            .samples::<f32>()
            .map(|sample| sample.unwrap())
            .collect();
        (spec, samples)
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn writes_stereo_float_wav() {
        let path = temp_wav_path("audio-file-sink");
        let (mut sink, handle) =
            AudioFileSink::new(path.clone(), 44_100, false, false, 16).unwrap();

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

    #[cfg(target_arch = "wasm32")]
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

    #[cfg(target_arch = "wasm32")]
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

    #[test]
    fn monitor_controls_sink_output() {
        #[cfg(not(target_arch = "wasm32"))]
        let path = temp_wav_path("audio-file-sink-monitor");
        #[cfg(not(target_arch = "wasm32"))]
        let (mut sink, handle) =
            AudioFileSink::new(path.clone(), 44_100, false, false, 16).unwrap();
        #[cfg(target_arch = "wasm32")]
        let (mut sink, handle) = AudioFileSink::new_wasm(44_100, false, false, 16).unwrap();

        sink.set_input("audio_left", 0.2).unwrap();
        sink.set_input("audio_right", 0.4).unwrap();
        sink.process();
        assert_eq!(sink.sink_output(), SinkOutput::default());

        sink = sink.with_monitor(true);
        assert_eq!(sink.sink_output(), SinkOutput::stereo(0.2, 0.4));

        handle.finish();
        #[cfg(not(target_arch = "wasm32"))]
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn soft_clip_can_be_disabled() {
        #[cfg(not(target_arch = "wasm32"))]
        let path = temp_wav_path("audio-file-sink-clip");
        #[cfg(not(target_arch = "wasm32"))]
        let (mut sink, handle) = AudioFileSink::new(path.clone(), 44_100, true, true, 16).unwrap();
        #[cfg(target_arch = "wasm32")]
        let (mut sink, handle) = AudioFileSink::new_wasm(44_100, true, true, 16).unwrap();

        sink.set_input("audio", 3.0).unwrap();
        sink.process();
        assert!(sink.sink_output().left < 1.0);

        sink = sink.with_soft_clip(false);
        sink.reset_inputs();
        sink.set_input("audio", 3.0).unwrap();
        sink.process();
        assert_eq!(sink.sink_output(), SinkOutput::stereo(3.0, 3.0));

        handle.finish();
        #[cfg(not(target_arch = "wasm32"))]
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn factory_exposes_ports_without_building() {
        let factory = AudioFileSinkFactory;
        assert!(factory.is_sink());
        assert_eq!(factory.input_ports(), Some(&inputs::INPUTS[..]));
        assert_eq!(factory.output_ports(), Some(&outputs::OUTPUTS[..]));
    }
}
