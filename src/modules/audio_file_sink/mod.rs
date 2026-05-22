//! Audio file sink module for recording graph audio to disk.

use std::any::Any;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use hound::{SampleFormat, WavSpec, WavWriter};

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::{Module, SinkModule, SinkOutput};

mod inputs;
mod outputs;

const DEFAULT_BUFFER_FRAMES: usize = 65_536;

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

        let (sink, handle) =
            AudioFileSink::new(path.into(), sample_rate, soft_clip, monitor, buffer_frames)?;

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

pub struct AudioFileSink {
    inputs: inputs::AudioFileSinkInputs,
    outputs: outputs::AudioFileSinkOutputs,
    shared: Arc<AudioFileSinkShared>,
    soft_clip: bool,
    monitor: bool,
    last_processed_sample: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFileSinkStats {
    pub frames_written: usize,
    pub frames_dropped: usize,
}

#[derive(Clone)]
pub struct AudioFileSinkHandle {
    shared: Arc<AudioFileSinkShared>,
}

struct AudioFileSinkShared {
    ring: AudioFrameRing,
    stopping: AtomicBool,
    frames_written: AtomicUsize,
    join_handle: Mutex<Option<JoinHandle<()>>>,
}

struct AudioFrameRing {
    slots: Box<[AtomicU64]>,
    read_index: AtomicUsize,
    write_index: AtomicUsize,
    capacity: usize,
    dropped: AtomicUsize,
}

impl AudioFileSink {
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

        let shared = Arc::new(AudioFileSinkShared {
            ring: AudioFrameRing::new(buffer_frames),
            stopping: AtomicBool::new(false),
            frames_written: AtomicUsize::new(0),
            join_handle: Mutex::new(None),
        });

        let worker_shared = shared.clone();
        let join_handle = thread::spawn(move || write_worker(worker_shared, writer));
        *shared.join_handle.lock().unwrap() = Some(join_handle);

        let sink = Self {
            inputs: inputs::AudioFileSinkInputs::new(),
            outputs: outputs::AudioFileSinkOutputs::new(),
            shared: shared.clone(),
            soft_clip,
            monitor,
            last_processed_sample: 0,
        };
        let handle = AudioFileSinkHandle { shared };
        Ok((sink, handle))
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
            self.shared.ring.push(left, right);
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
        if let Some(join_handle) = self.shared.join_handle.lock().unwrap().take() {
            let _ = join_handle.join();
        }
        self.stats()
    }

    pub fn stats(&self) -> AudioFileSinkStats {
        AudioFileSinkStats {
            frames_written: self.shared.frames_written.load(Ordering::Acquire),
            frames_dropped: self.shared.ring.dropped.load(Ordering::Acquire),
        }
    }
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

fn write_worker(shared: Arc<AudioFileSinkShared>, mut writer: WavWriter<BufWriter<File>>) {
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
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_wav_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("fugue-{}-{}.wav", name, nanos))
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

    #[test]
    fn monitor_controls_sink_output() {
        let path = temp_wav_path("audio-file-sink-monitor");
        let (mut sink, handle) =
            AudioFileSink::new(path.clone(), 44_100, false, false, 16).unwrap();

        sink.set_input("audio_left", 0.2).unwrap();
        sink.set_input("audio_right", 0.4).unwrap();
        sink.process();
        assert_eq!(sink.sink_output(), SinkOutput::default());

        sink = sink.with_monitor(true);
        assert_eq!(sink.sink_output(), SinkOutput::stereo(0.2, 0.4));

        handle.finish();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn soft_clip_can_be_disabled() {
        let path = temp_wav_path("audio-file-sink-clip");
        let (mut sink, handle) = AudioFileSink::new(path.clone(), 44_100, true, true, 16).unwrap();

        sink.set_input("audio", 3.0).unwrap();
        sink.process();
        assert!(sink.sink_output().left < 1.0);

        sink = sink.with_soft_clip(false);
        sink.reset_inputs();
        sink.set_input("audio", 3.0).unwrap();
        sink.process();
        assert_eq!(sink.sink_output(), SinkOutput::stereo(3.0, 3.0));

        handle.finish();
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
