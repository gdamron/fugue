//! Thread-safe controls for the SamplePlayer module.

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex, OnceLock};

use crate::atomic::AtomicF32;
use crate::{ControlMeta, ControlSurface, ControlValue};

#[derive(Clone)]
pub struct SamplePlayerControls {
    pub(crate) shared: Arc<Mutex<SamplePlayerShared>>,
    pitch_ratio: AtomicF32,
    sample_rate: u32,
}

pub(crate) struct SamplePlayerShared {
    pub(crate) source: String,
    pub(crate) play: bool,
    pub(crate) loop_enabled: bool,
    pub(crate) play_trigger: u64,
    pub(crate) pending_sample: Option<Arc<SampleData>>,
}

pub(crate) struct SampleData {
    pub(crate) left: Vec<f32>,
    pub(crate) right: Vec<f32>,
}

impl SampleData {
    pub(crate) fn len(&self) -> usize {
        self.left.len().min(self.right.len())
    }

    /// Reads both channels at fractional frame position `pos` using the shared
    /// cubic interpolation kernel. Used for audio-rate pitched playback.
    #[inline]
    pub(crate) fn sample_at(&self, pos: f64) -> (f32, f32) {
        (
            cubic_sample(&self.left, pos),
            cubic_sample(&self.right, pos),
        )
    }

    fn from_interleaved(
        channels: usize,
        sample_rate: u32,
        target_sample_rate: u32,
        samples: Vec<f32>,
    ) -> Self {
        if channels == 0 {
            return Self {
                left: Vec::new(),
                right: Vec::new(),
            };
        }

        let frames = samples.len() / channels;
        let mut left = Vec::with_capacity(frames);
        let mut right = Vec::with_capacity(frames);

        for frame in samples.chunks(channels) {
            let l = frame.first().copied().unwrap_or(0.0);
            let r = frame.get(1).copied().unwrap_or(l);
            left.push(l);
            right.push(r);
        }

        if sample_rate == target_sample_rate || left.is_empty() {
            return Self { left, right };
        }

        Self {
            left: resample_channel(&left, sample_rate, target_sample_rate),
            right: resample_channel(&right, sample_rate, target_sample_rate),
        }
    }
}

impl SamplePlayerControls {
    pub fn new(
        sample_rate: u32,
        source: Option<&str>,
        play: Option<bool>,
        loop_enabled: Option<bool>,
    ) -> Result<Self, String> {
        let controls = Self {
            shared: Arc::new(Mutex::new(SamplePlayerShared {
                source: String::new(),
                play: false,
                loop_enabled: false,
                play_trigger: 0,
                pending_sample: None,
            })),
            pitch_ratio: AtomicF32::new(1.0),
            sample_rate,
        };

        if let Some(source) = source {
            if !source.is_empty() {
                controls.set_source(source)?;
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

    pub fn source(&self) -> String {
        self.shared.lock().unwrap().source.clone()
    }

    pub fn set_source(&self, source: &str) -> Result<(), String> {
        let target = resolve_source(source)?;
        let sample = load_cached_sample(&target, self.sample_rate)?;
        let mut shared = self.shared.lock().unwrap();
        // The authored ref stays the control value, so a saved document keeps
        // the portable form rather than this machine's cache path.
        shared.source = source.to_string();
        shared.pending_sample = Some(sample);
        Ok(())
    }

    pub fn play(&self) -> bool {
        self.shared.lock().unwrap().play
    }

    pub fn set_play(&self, play: bool) {
        let mut shared = self.shared.lock().unwrap();
        shared.play = play;
        if play {
            shared.play_trigger = shared.play_trigger.wrapping_add(1);
        }
    }

    pub fn loop_enabled(&self) -> bool {
        self.shared.lock().unwrap().loop_enabled
    }

    pub fn set_loop_enabled(&self, loop_enabled: bool) {
        self.shared.lock().unwrap().loop_enabled = loop_enabled;
    }

    pub fn pitch_ratio(&self) -> f32 {
        self.pitch_ratio.load()
    }

    pub fn set_pitch_ratio(&self, pitch_ratio: f32) {
        // Clamp to a small positive floor so the read head always advances
        // forward; a zero or negative ratio would stall or reverse playback.
        self.pitch_ratio.store(pitch_ratio.max(1e-4));
    }
}

impl ControlSurface for SamplePlayerControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::string(
                "source",
                "Audio sample path, https URL, or package ref like \
                 'fugue.drums.808@1.2.0:kick/long.wav' (WAV or FLAC)",
            )
            .with_default(self.source()),
            ControlMeta::boolean("play", "Start or stop sample playback", self.play()),
            ControlMeta::boolean("loop", "Loop playback when enabled", self.loop_enabled()),
            ControlMeta::number(
                "pitch_ratio",
                "Playback speed / pitch ratio (1.0 = native, 2.0 = up an octave)",
            )
            .with_range(0.25, 4.0)
            .with_default(self.pitch_ratio()),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "source" => Ok(self.source().into()),
            "play" => Ok(self.play().into()),
            "loop" => Ok(self.loop_enabled().into()),
            "pitch_ratio" => Ok(self.pitch_ratio().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "source" => self.set_source(value.as_string()?)?,
            "play" => self.set_play(value.as_bool()?),
            "loop" => self.set_loop_enabled(value.as_bool()?),
            "pitch_ratio" => self.set_pitch_ratio(value.as_number()?),
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }
}

/// Package refs (`id@requirement:file`) resolve through the installed package
/// cache; any other string loads unchanged as a path or https URL. Note the
/// resample cache below is keyed by the resolved path, so two spellings of
/// the same sample share one buffer.
fn resolve_source(source: &str) -> Result<String, String> {
    let Some(reference) = crate::pkg::PackageAudioRef::parse(source) else {
        return Ok(source.to_string());
    };

    #[cfg(target_arch = "wasm32")]
    {
        Err(format!(
            "package asset ref '{}' is not available on wasm32",
            reference
        ))
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let packages_dir = crate::pkg::default_packages_dir()?;
        let resolved = crate::pkg::resolve_package_asset(&reference, &packages_dir, None)?;
        Ok(resolved.file.to_string_lossy().into_owned())
    }
}

/// Audio container formats the sample player can decode.
enum AudioFormat {
    Wav,
    Flac,
}

/// Process-wide cache of decoded + resampled buffers, keyed by the resolved
/// source string and the target (engine) sample rate. Decode/resample happens
/// off the audio thread, so a `Mutex` here is fine; the audio thread only ever
/// touches the resulting `Arc<SampleData>`. Deduplicates work and memory when
/// several players reference the same asset at the same rate.
//
// Keyed by the *resolved* source path (`resolve_source`), so a package ref
// and the direct path to the same installed file share one buffer.
type SampleCache = Mutex<HashMap<(String, u32), Arc<SampleData>>>;

fn sample_cache() -> &'static SampleCache {
    static CACHE: OnceLock<SampleCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Returns the resampled buffer for `(source, target_sample_rate)`, decoding it
/// (and caching) only on a miss. A decode error leaves the cache untouched.
fn load_cached_sample(source: &str, target_sample_rate: u32) -> Result<Arc<SampleData>, String> {
    let key = (source.to_string(), target_sample_rate);

    if let Some(cached) = sample_cache().lock().unwrap().get(&key) {
        return Ok(Arc::clone(cached));
    }

    let sample = Arc::new(load_sample(source, target_sample_rate)?);
    sample_cache()
        .lock()
        .unwrap()
        .insert(key, Arc::clone(&sample));
    Ok(sample)
}

fn load_sample(source: &str, target_sample_rate: u32) -> Result<SampleData, String> {
    let (mut reader, remote) = open_source(source)?;

    // Peek the header magic so format detection is header-driven (authoritative)
    // with the file extension as a fallback. The source readers (file or HTTPS
    // stream) are not seekable, so the consumed bytes are chained back on.
    let mut magic = [0u8; 4];
    let filled = read_magic(&mut reader, &mut magic)?;
    let format = detect_format(source, &magic[..filled])?;
    let reader = std::io::Cursor::new(magic[..filled].to_vec()).chain(reader);

    let (channels, sample_rate, samples) = match format {
        AudioFormat::Wav => decode_wav(reader)?,
        AudioFormat::Flac => decode_flac(reader)?,
    };
    let data = SampleData::from_interleaved(channels, sample_rate, target_sample_rate, samples);

    if data.len() == 0 {
        let location = if remote { "URL" } else { "file" };
        return Err(format!("Decoded empty sample from {}", location));
    }

    Ok(data)
}

/// Reads up to `buf.len()` header bytes, returning how many were read. Short
/// reads are tolerated so detection can fall back to the file extension.
fn read_magic<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<usize, String> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(err) => return Err(format!("Failed to read audio header: {}", err)),
        }
    }
    Ok(filled)
}

fn detect_format(source: &str, magic: &[u8]) -> Result<AudioFormat, String> {
    if magic.starts_with(b"RIFF") {
        return Ok(AudioFormat::Wav);
    }
    if magic.starts_with(b"fLaC") {
        return Ok(AudioFormat::Flac);
    }

    match source_extension(source).as_deref() {
        Some("wav") => Ok(AudioFormat::Wav),
        Some("flac") => Ok(AudioFormat::Flac),
        _ => Err(format!(
            "Unsupported audio format for '{}': expected WAV or FLAC",
            source
        )),
    }
}

/// Lowercased file extension of `source`, ignoring any URL query/fragment.
fn source_extension(source: &str) -> Option<String> {
    let path = source.split(['?', '#']).next().unwrap_or(source);
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let (_, ext) = name.rsplit_once('.')?;
    if ext.is_empty() {
        None
    } else {
        Some(ext.to_ascii_lowercase())
    }
}

fn open_source(source: &str) -> Result<(Box<dyn Read>, bool), String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = source;
        Err("Sample loading is not available on wasm32".to_string())
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        if source.starts_with("https://") {
            let response = ureq::get(source)
                .call()
                .map_err(|err| format!("Failed to download sample: {}", err))?;
            return Ok((Box::new(response.into_reader()), true));
        }

        if source.starts_with("http://") {
            return Err("Only https:// URLs are supported".to_string());
        }

        let file = std::fs::File::open(source)
            .map_err(|err| format!("Failed to open sample '{}': {}", source, err))?;
        Ok((Box::new(file), false))
    }
}

/// Decodes a WAV stream into `(channels, sample_rate, interleaved f32 samples)`.
fn decode_wav<R: Read>(reader: R) -> Result<(usize, u32, Vec<f32>), String> {
    let mut wav =
        hound::WavReader::new(reader).map_err(|err| format!("Failed to decode WAV: {}", err))?;
    let spec = wav.spec();
    let channels = spec.channels.max(1) as usize;
    let sample_rate = spec.sample_rate;
    let samples = match spec.sample_format {
        hound::SampleFormat::Float => wav
            .samples::<f32>()
            .map(|sample| sample.map_err(|err| err.to_string()))
            .collect::<Result<Vec<f32>, String>>()?,
        hound::SampleFormat::Int => {
            let shift = spec.bits_per_sample.saturating_sub(1) as u32;
            let scale = (1_i64 << shift) as f32;
            wav.samples::<i32>()
                .map(|sample| {
                    sample
                        .map(|value| (value as f32 / scale).clamp(-1.0, 1.0))
                        .map_err(|err| err.to_string())
                })
                .collect::<Result<Vec<f32>, String>>()?
        }
    };
    Ok((channels, sample_rate, samples))
}

/// Decodes a FLAC stream into `(channels, sample_rate, interleaved f32 samples)`,
/// matching the normalization used for integer WAV so both formats share the
/// downstream contract.
#[cfg(not(target_arch = "wasm32"))]
fn decode_flac<R: Read>(reader: R) -> Result<(usize, u32, Vec<f32>), String> {
    let mut flac =
        claxon::FlacReader::new(reader).map_err(|err| format!("Failed to decode FLAC: {}", err))?;
    let info = flac.streaminfo();
    let channels = info.channels.max(1) as usize;
    let sample_rate = info.sample_rate;
    let shift = info.bits_per_sample.saturating_sub(1);
    let scale = (1_i64 << shift) as f32;
    let samples = flac
        .samples()
        .map(|sample| {
            sample
                .map(|value| (value as f32 / scale).clamp(-1.0, 1.0))
                .map_err(|err| err.to_string())
        })
        .collect::<Result<Vec<f32>, String>>()?;
    Ok((channels, sample_rate, samples))
}

#[cfg(target_arch = "wasm32")]
fn decode_flac<R: Read>(_reader: R) -> Result<(usize, u32, Vec<f32>), String> {
    Err("FLAC decoding is not available on wasm32".to_string())
}

fn resample_channel(input: &[f32], sample_rate: u32, target_sample_rate: u32) -> Vec<f32> {
    if input.is_empty() {
        return Vec::new();
    }

    let target_len = ((input.len() as f64 * target_sample_rate as f64) / sample_rate as f64)
        .round()
        .max(1.0) as usize;
    let ratio = sample_rate as f64 / target_sample_rate as f64;
    let mut output = Vec::with_capacity(target_len);

    for index in 0..target_len {
        let source_pos = index as f64 * ratio;
        output.push(cubic_sample(input, source_pos));
    }

    output
}

/// 4-point (Catmull-Rom) cubic Hermite interpolation between `y1` and `y2`,
/// where `t` is the fractional position in `[0, 1)`.
#[inline]
fn cubic_hermite(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c0 = y1;
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + c0
}

/// Reads `input` at fractional position `pos`, using the four samples
/// surrounding `pos` with edge-clamped neighbours. Allocation-free; the shared
/// kernel for both on-load sample-rate conversion and audio-rate pitch.
#[inline]
pub(crate) fn cubic_sample(input: &[f32], pos: f64) -> f32 {
    let n = input.len();
    if n == 0 {
        return 0.0;
    }
    let base = pos.floor() as isize;
    let frac = (pos - base as f64) as f32;
    let at = |i: isize| input[i.clamp(0, n as isize - 1) as usize];
    cubic_hermite(at(base - 1), at(base), at(base + 1), at(base + 2), frac)
}
