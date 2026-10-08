//! Native adapter between [`RtmpSink`](super::RtmpSink) and the shared ffmpeg backend.

use std::path::PathBuf;
use std::sync::Arc;

use crate::streaming::ffmpeg::{
    FfmpegStreamBackend, FfmpegStreamConfig, FfmpegStreamHandle, FfmpegStreamStats,
    DEFAULT_AUDIO_BUFFER_FRAMES, DEFAULT_VIDEO_QUEUE_FRAMES,
};
use crate::streaming::video::{
    BlackVideoFallback, BlackVideoFallbackHandle, VideoFrameTarget, VideoPlayback,
    VideoPlaybackConfig, VideoPlaybackHandle,
};

use super::{RtmpSink, RtmpSinkHandle, RtmpSinkStats};
use crate::module_config::{ConfigKey, ConfigReader};

const WIDTH: ConfigKey = ConfigKey::int::<u32>("width");
const HEIGHT: ConfigKey = ConfigKey::int::<u32>("height");
const FPS: ConfigKey = ConfigKey::int::<u32>("fps");
const GOP_SECONDS: ConfigKey = ConfigKey::int::<u32>("gop_seconds");
const BUFFER_FRAMES: ConfigKey = ConfigKey::int::<usize>("buffer_frames");
const VIDEO_QUEUE_FRAMES: ConfigKey = ConfigKey::int::<usize>("video_queue_frames");
// Whole kbps, or text such as "2500k".
const VIDEO_BITRATE: ConfigKey = ConfigKey::int::<u64>("video_bitrate");
const AUDIO_BITRATE: ConfigKey = ConfigKey::int::<u64>("audio_bitrate");

/// The keys of `rtmp_sink` config, then `youtube_sink`'s own: `youtube_sink`
/// takes all but `url` (the first), and `rtmp_sink` all but the last three.
pub(crate) const CONFIG_KEYS: &[ConfigKey] = &[
    ConfigKey::text("url"),
    WIDTH,
    HEIGHT,
    FPS,
    GOP_SECONDS,
    BUFFER_FRAMES,
    VIDEO_QUEUE_FRAMES,
    VIDEO_BITRATE,
    AUDIO_BITRATE,
    ConfigKey::text("resolution"),
    ConfigKey::text("ffmpeg_path"),
    ConfigKey::text("tee_to_disk"),
    ConfigKey::text("video_encoder"),
    ConfigKey::text("audio_encoder"),
    ConfigKey::boolean("monitor"),
    ConfigKey::boolean("soft_clip"),
    ConfigKey::text("background_video"),
    ConfigKey::text("stream_key"),
    ConfigKey::text("stream_key_env"),
    ConfigKey::text("server_url"),
];

#[derive(Clone)]
pub(super) struct SharedHandle {
    stream: FfmpegStreamHandle,
    background_video: Option<VideoPlaybackHandle>,
    black_video_fallback: Option<BlackVideoFallbackHandle>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtmpSinkConfig {
    pub ffmpeg_path: String,
    pub url: String,
    pub tee_to_disk: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub sample_rate: u32,
    pub video_encoder: String,
    pub audio_encoder: String,
    pub video_bitrate: String,
    pub audio_bitrate: String,
    pub gop_seconds: u32,
    pub buffer_frames: usize,
    pub video_queue_frames: usize,
    pub monitor: bool,
    pub soft_clip: bool,
    pub background_video: Option<String>,
}

pub(super) fn build(
    config: &serde_json::Value,
    sample_rate: u32,
) -> Result<(RtmpSink, RtmpSinkHandle), Box<dyn std::error::Error>> {
    let config = RtmpSinkConfig::from_json(config, sample_rate)?;
    FfmpegStreamBackend::validate_ffmpeg(&config.ffmpeg_path)?;
    RtmpSink::new_native(config)
}

impl RtmpSinkConfig {
    pub(crate) fn from_json(
        config: &serde_json::Value,
        sample_rate: u32,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::from_json_as("rtmp_sink", config, sample_rate)
    }

    /// Like [`Self::from_json`], naming `module_type` in number refusals.
    pub(crate) fn from_json_as(
        module_type: &str,
        config: &serde_json::Value,
        sample_rate: u32,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let config = &ConfigReader::new(module_type, config);
        let url = required_string(config, "url")?;
        let (width, height) = parse_dimensions(config)?;
        if width == 0 || height == 0 {
            return Err("rtmp_sink width and height must be greater than zero".into());
        }

        let fps = optional_u32(config, &FPS, 30)?;
        if fps == 0 {
            return Err("rtmp_sink fps must be greater than zero".into());
        }

        let gop_seconds = optional_u32(config, &GOP_SECONDS, 2)?;
        if gop_seconds == 0 {
            return Err("rtmp_sink gop_seconds must be greater than zero".into());
        }

        let buffer_frames = optional_usize(config, &BUFFER_FRAMES, DEFAULT_AUDIO_BUFFER_FRAMES)?;
        if buffer_frames == 0 {
            return Err("rtmp_sink buffer_frames must be greater than zero".into());
        }

        let video_queue_frames =
            optional_usize(config, &VIDEO_QUEUE_FRAMES, DEFAULT_VIDEO_QUEUE_FRAMES)?;
        if video_queue_frames == 0 {
            return Err("rtmp_sink video_queue_frames must be greater than zero".into());
        }

        Ok(Self {
            ffmpeg_path: optional_string(config, "ffmpeg_path", "ffmpeg"),
            url,
            tee_to_disk: optional_nonempty_string_value(config, "tee_to_disk")?,
            width,
            height,
            fps,
            sample_rate,
            video_encoder: optional_string(config, "video_encoder", "libx264"),
            audio_encoder: optional_string(config, "audio_encoder", "aac"),
            video_bitrate: optional_bitrate(config, &VIDEO_BITRATE, "2500k")?,
            audio_bitrate: optional_bitrate(config, &AUDIO_BITRATE, "128k")?,
            gop_seconds,
            buffer_frames,
            video_queue_frames,
            monitor: optional_bool(config, "monitor", false),
            soft_clip: optional_bool(config, "soft_clip", true),
            background_video: optional_string_value(config, "background_video")?,
        })
    }

    fn stream_config(&self, constant_video_bitrate: bool) -> FfmpegStreamConfig {
        FfmpegStreamConfig {
            ffmpeg_path: self.ffmpeg_path.clone(),
            url: self.url.clone(),
            tee_to_disk: self.tee_to_disk.clone(),
            width: self.width,
            height: self.height,
            fps: self.fps,
            sample_rate: self.sample_rate,
            video_encoder: self.video_encoder.clone(),
            audio_encoder: self.audio_encoder.clone(),
            video_bitrate: self.video_bitrate.clone(),
            audio_bitrate: self.audio_bitrate.clone(),
            gop_seconds: self.gop_seconds,
            constant_video_bitrate,
            audio_buffer_frames: self.buffer_frames,
            video_queue_frames: self.video_queue_frames,
        }
    }

    fn video_playback_config(&self) -> Option<VideoPlaybackConfig> {
        self.background_video
            .as_ref()
            .map(|path| VideoPlaybackConfig {
                ffmpeg_path: self.ffmpeg_path.clone(),
                path: PathBuf::from(path),
                width: self.width,
                height: self.height,
                fps: self.fps,
                autoplay: true,
                loop_enabled: true,
            })
    }
}

impl RtmpSink {
    pub fn new_native(
        config: RtmpSinkConfig,
    ) -> Result<(Self, RtmpSinkHandle), Box<dyn std::error::Error>> {
        Self::new_native_with_options(config, false, false)
    }

    pub(crate) fn new_native_with_options(
        config: RtmpSinkConfig,
        constant_video_bitrate: bool,
        black_video_fallback: bool,
    ) -> Result<(Self, RtmpSinkHandle), Box<dyn std::error::Error>> {
        let soft_clip = config.soft_clip;
        let monitor = config.monitor;
        let stream = FfmpegStreamBackend::start(config.stream_config(constant_video_bitrate))?;
        let background_video = if let Some(playback_config) = config.video_playback_config() {
            let target_stream = stream.clone();
            let target: VideoFrameTarget =
                Arc::new(move |frame| target_stream.push_video_rgba(frame));
            match VideoPlayback::start(playback_config, target) {
                Ok(playback) => Some(playback),
                Err(err) => {
                    stream.finish();
                    return Err(err);
                }
            }
        } else {
            None
        };
        let black_video_fallback = if black_video_fallback && background_video.is_none() {
            let target_stream = stream.clone();
            let target: VideoFrameTarget =
                Arc::new(move |frame| target_stream.push_video_rgba(frame));
            match BlackVideoFallback::start(config.width, config.height, config.fps, target) {
                Ok(fallback) => Some(fallback),
                Err(err) => {
                    stream.finish();
                    return Err(err);
                }
            }
        } else {
            None
        };
        let shared = SharedHandle {
            stream,
            background_video,
            black_video_fallback,
        };
        Ok(Self::from_shared(shared, soft_clip, monitor))
    }
}

impl SharedHandle {
    #[inline]
    pub(super) fn push_audio(&self, left: f32, right: f32) {
        self.stream.push_audio(left, right);
    }

    #[inline]
    pub(super) fn is_stopping(&self) -> bool {
        self.stream.is_stopping()
    }

    pub(super) fn push_video_rgba(&self, frame: &[u8]) -> Result<(), String> {
        self.stream.push_video_rgba(frame)?;
        if let Some(fallback) = &self.black_video_fallback {
            fallback.external_video_started();
        }
        Ok(())
    }

    pub(super) fn has_background_video(&self) -> bool {
        self.background_video.is_some()
    }

    pub(super) fn play_background_video(&self) -> Result<(), String> {
        self.background_video
            .as_ref()
            .ok_or_else(|| "rtmp_sink has no background video".to_string())?
            .play();
        Ok(())
    }

    pub(super) fn pause_background_video(&self) -> Result<(), String> {
        self.background_video
            .as_ref()
            .ok_or_else(|| "rtmp_sink has no background video".to_string())?
            .pause();
        Ok(())
    }

    pub(super) fn restart_background_video(&self) -> Result<(), String> {
        self.background_video
            .as_ref()
            .ok_or_else(|| "rtmp_sink has no background video".to_string())?
            .restart();
        Ok(())
    }

    pub(super) fn set_background_video_loop(&self, enabled: bool) -> Result<(), String> {
        self.background_video
            .as_ref()
            .ok_or_else(|| "rtmp_sink has no background video".to_string())?
            .set_loop_enabled(enabled);
        Ok(())
    }

    pub(super) fn stop(&self) {
        if let Some(background_video) = &self.background_video {
            background_video.stop();
        }
        if let Some(fallback) = &self.black_video_fallback {
            fallback.stop();
        }
        self.stream.stop();
    }

    pub(super) fn finish(&self) {
        if let Some(background_video) = &self.background_video {
            background_video.finish();
        }
        if let Some(fallback) = &self.black_video_fallback {
            fallback.finish();
        }
        self.stream.finish();
    }

    pub(super) fn stats(&self) -> FfmpegStreamStats {
        let mut stats = self.stream.stats();
        if stats.last_error.is_none() {
            stats.last_error = self
                .background_video
                .as_ref()
                .and_then(|video| video.stats().last_error);
        }
        if stats.last_error.is_none() {
            stats.last_error = self
                .black_video_fallback
                .as_ref()
                .and_then(BlackVideoFallbackHandle::last_error);
        }
        stats
    }
}

impl From<FfmpegStreamStats> for RtmpSinkStats {
    fn from(stats: FfmpegStreamStats) -> Self {
        Self {
            audio_frames_sent: stats.audio_frames_sent,
            audio_frames_dropped: stats.audio_frames_dropped,
            video_frames_sent: stats.video_frames_sent,
            video_frames_dropped: stats.video_frames_dropped,
            restarts: stats.restarts,
            last_error: stats.last_error,
        }
    }
}

fn required_string(config: &ConfigReader, key: &str) -> Result<String, Box<dyn std::error::Error>> {
    config
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("rtmp_sink requires config.{key}").into())
}

fn parse_dimensions(config: &ConfigReader) -> Result<(u32, u32), Box<dyn std::error::Error>> {
    match (config.get("width"), config.get("height")) {
        (Some(_), Some(_)) => Ok((
            required_u32(config, &WIDTH)?,
            required_u32(config, &HEIGHT)?,
        )),
        (None, None) => parse_resolution(config),
        _ => Err(
            "rtmp_sink requires both config.width and config.height, or config.resolution".into(),
        ),
    }
}

fn parse_resolution(config: &ConfigReader) -> Result<(u32, u32), Box<dyn std::error::Error>> {
    let resolution = config
        .get("resolution")
        .and_then(|value| value.as_str())
        .ok_or("rtmp_sink requires config.width/config.height or config.resolution")?;
    let (width, height) = resolution
        .split_once('x')
        .or_else(|| resolution.split_once('X'))
        .ok_or("rtmp_sink resolution must look like 1920x1080")?;
    let width = width
        .parse::<u32>()
        .map_err(|_| "rtmp_sink resolution width must be a positive integer")?;
    let height = height
        .parse::<u32>()
        .map_err(|_| "rtmp_sink resolution height must be a positive integer")?;
    Ok((width, height))
}

fn optional_string(config: &ConfigReader, key: &str, default: &str) -> String {
    config
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or(default)
        .to_string()
}

fn optional_string_value(
    config: &ConfigReader,
    key: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    match config.get(key) {
        Some(value) if value.is_string() => Ok(value.as_str().map(str::to_string)),
        Some(_) => Err(format!("rtmp_sink config.{key} must be a string").into()),
        None => Ok(None),
    }
}

fn optional_nonempty_string_value(
    config: &ConfigReader,
    key: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    match optional_string_value(config, key)? {
        Some(value) if value.trim().is_empty() => {
            Err(format!("rtmp_sink config.{key} must not be empty").into())
        }
        value => Ok(value),
    }
}

/// A bitrate as text (`"2500k"`) or as whole kbps.
fn optional_bitrate(
    config: &ConfigReader,
    key: &ConfigKey,
    default: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    match config.get(key.key) {
        Some(serde_json::Value::String(text)) => Ok(text.clone()),
        Some(serde_json::Value::Number(_)) | None => Ok(config
            .int::<u64>(key)?
            .map_or_else(|| default.to_string(), |kbps| format!("{kbps}k"))),
        Some(_) => Err(format!(
            "rtmp_sink config.{} must be a string or integer kbps",
            key.key
        )
        .into()),
    }
}

fn required_u32(config: &ConfigReader, key: &ConfigKey) -> Result<u32, Box<dyn std::error::Error>> {
    let value = config.int::<u32>(key)?;
    Ok(value.ok_or_else(|| format!("rtmp_sink requires numeric config.{}", key.key))?)
}

fn optional_u32(
    config: &ConfigReader,
    key: &ConfigKey,
    default: u32,
) -> Result<u32, Box<dyn std::error::Error>> {
    Ok(config.int::<u32>(key)?.unwrap_or(default))
}

fn optional_usize(
    config: &ConfigReader,
    key: &ConfigKey,
    default: usize,
) -> Result<usize, Box<dyn std::error::Error>> {
    Ok(config.int::<usize>(key)?.unwrap_or(default))
}

fn optional_bool(config: &ConfigReader, key: &str, default: bool) -> bool {
    config
        .get(key)
        .and_then(|value| value.as_bool())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn minimal_config() -> serde_json::Value {
        json!({
            "url": "rtmp://example.test/live/fugue",
            "width": 640,
            "height": 360
        })
    }

    #[test]
    fn parses_defaults() {
        let config = RtmpSinkConfig::from_json(&minimal_config(), 48_000).unwrap();

        assert_eq!(config.ffmpeg_path, "ffmpeg");
        assert_eq!(config.url, "rtmp://example.test/live/fugue");
        assert_eq!(config.tee_to_disk, None);
        assert_eq!(config.width, 640);
        assert_eq!(config.height, 360);
        assert_eq!(config.fps, 30);
        assert_eq!(config.sample_rate, 48_000);
        assert_eq!(config.video_encoder, "libx264");
        assert_eq!(config.audio_encoder, "aac");
        assert_eq!(config.video_bitrate, "2500k");
        assert_eq!(config.audio_bitrate, "128k");
        assert_eq!(config.gop_seconds, 2);
        assert_eq!(config.buffer_frames, DEFAULT_AUDIO_BUFFER_FRAMES);
        assert_eq!(config.video_queue_frames, DEFAULT_VIDEO_QUEUE_FRAMES);
        assert!(!config.monitor);
        assert!(config.soft_clip);
        assert_eq!(config.background_video, None);
    }

    #[test]
    fn parses_fug_137_config_aliases() {
        let config = RtmpSinkConfig::from_json(
            &json!({
                "url": "rtmp://example.test/live/fugue",
                "resolution": "1920x1080",
                "video_bitrate": 4500,
                "audio_bitrate": 128,
                "fps": 60,
                "background_video": "./loop.mp4",
                "tee_to_disk": "./broadcast.mkv"
            }),
            48_000,
        )
        .unwrap();

        assert_eq!(config.width, 1920);
        assert_eq!(config.height, 1080);
        assert_eq!(config.video_bitrate, "4500k");
        assert_eq!(config.audio_bitrate, "128k");
        assert_eq!(config.fps, 60);
        assert_eq!(config.background_video.as_deref(), Some("./loop.mp4"));
        assert_eq!(config.tee_to_disk.as_deref(), Some("./broadcast.mkv"));
    }

    #[test]
    fn rejects_invalid_required_values() {
        let missing_url = json!({ "width": 640, "height": 360 });
        assert!(RtmpSinkConfig::from_json(&missing_url, 48_000)
            .unwrap_err()
            .to_string()
            .contains("config.url"));

        let zero_width = json!({ "url": "rtmp://x", "width": 0, "height": 360 });
        assert!(RtmpSinkConfig::from_json(&zero_width, 48_000)
            .unwrap_err()
            .to_string()
            .contains("width and height"));

        let zero_fps = json!({ "url": "rtmp://x", "width": 640, "height": 360, "fps": 0 });
        assert!(RtmpSinkConfig::from_json(&zero_fps, 48_000)
            .unwrap_err()
            .to_string()
            .contains("fps"));

        let partial_dimensions = json!({ "url": "rtmp://x", "width": 640 });
        assert!(RtmpSinkConfig::from_json(&partial_dimensions, 48_000)
            .unwrap_err()
            .to_string()
            .contains("width and config.height"));

        let bad_resolution = json!({ "url": "rtmp://x", "resolution": "nope" });
        assert!(RtmpSinkConfig::from_json(&bad_resolution, 48_000)
            .unwrap_err()
            .to_string()
            .contains("resolution"));

        let bad_bitrate = json!({
            "url": "rtmp://x",
            "width": 640,
            "height": 360,
            "audio_bitrate": true
        });
        assert!(RtmpSinkConfig::from_json(&bad_bitrate, 48_000)
            .unwrap_err()
            .to_string()
            .contains("audio_bitrate"));

        let bad_background_video = json!({
            "url": "rtmp://x",
            "width": 640,
            "height": 360,
            "background_video": true
        });
        assert!(RtmpSinkConfig::from_json(&bad_background_video, 48_000)
            .unwrap_err()
            .to_string()
            .contains("background_video"));

        let empty_tee_path = json!({
            "url": "rtmp://x",
            "width": 640,
            "height": 360,
            "tee_to_disk": "  "
        });
        assert!(RtmpSinkConfig::from_json(&empty_tee_path, 48_000)
            .unwrap_err()
            .to_string()
            .contains("tee_to_disk"));
    }

    #[test]
    fn numbers_read_whole_floats_and_are_refused_by_name() {
        let mut config = minimal_config();
        config["fps"] = json!(60.0);
        config["video_bitrate"] = json!(4500.0);
        config["audio_bitrate"] = json!("96k");
        let parsed = RtmpSinkConfig::from_json(&config, 48_000).unwrap();
        assert_eq!(parsed.fps, 60);
        assert_eq!(parsed.video_bitrate, "4500k");
        assert_eq!(parsed.audio_bitrate, "96k");

        for (key, value) in [("width", json!(640.5)), ("audio_bitrate", json!(128.5))] {
            let mut config = minimal_config();
            config[key] = value;
            let error = RtmpSinkConfig::from_json(&config, 48_000).unwrap_err();
            let expected = format!("rtmp_sink config '{key}' expects a whole number");
            assert!(error.to_string().starts_with(&expected), "{error}");
        }
    }

    #[test]
    fn missing_ffmpeg_error_is_clear() {
        let err =
            FfmpegStreamBackend::validate_ffmpeg("fugue-definitely-missing-ffmpeg").unwrap_err();
        assert!(err.to_string().contains("requires ffmpeg in PATH"));
    }

    #[cfg(unix)]
    mod unix_fake_ffmpeg {
        use super::*;
        use crate::{Module, SinkModule};
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::path::PathBuf;
        use std::time::{SystemTime, UNIX_EPOCH};

        fn temp_dir(name: &str) -> PathBuf {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            std::env::temp_dir().join(format!("fugue-rtmp-sink-module-{name}-{nanos}"))
        }

        fn fake_ffmpeg() -> PathBuf {
            let dir = temp_dir("passthrough");
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join("ffmpeg");
            let script = r#"#!/bin/sh
if [ "$1" = "-version" ]; then
  exit 0
fi
python3 - "$@" <<'PY'
import re
import select
import socket
import sys
import time

urls = [arg for arg in sys.argv[1:] if arg.startswith("tcp://")]
if len(urls) < 2:
    sys.stdout.buffer.write(bytes([64]) * 16)
    sys.stdout.flush()
    sys.exit(0)

def port(url):
    match = re.search(r":(\d+)(?:\?|$)", url)
    if not match:
        sys.exit(11)
    return int(match.group(1))

sockets = []
for url in urls[:2]:
    s = socket.create_connection(("127.0.0.1", port(url)), timeout=5)
    s.setblocking(False)
    sockets.append(s)

deadline = time.time() + 5
while sockets and time.time() < deadline:
    readable, _, _ = select.select(sockets, [], [], 0.05)
    for s in readable:
        data = s.recv(65536)
        if not data:
            sockets.remove(s)
            s.close()

sys.exit(0 if not sockets else 12)
PY
"#;
            fs::write(&path, script).unwrap();
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
            path
        }

        #[test]
        fn soft_clip_does_not_change_downstream_pass_through() {
            let fake = fake_ffmpeg();
            let config = RtmpSinkConfig::from_json(
                &json!({
                    "ffmpeg_path": fake.to_string_lossy(),
                    "url": "rtmp://example.test/live/fugue",
                    "width": 2,
                    "height": 2,
                    "buffer_frames": 64,
                    "video_queue_frames": 2,
                    "monitor": true,
                    "soft_clip": true
                }),
                48_000,
            )
            .unwrap();
            let (mut sink, handle) = RtmpSink::new_native(config).unwrap();

            sink.set_input("audio_left", 2.0).unwrap();
            sink.set_input("audio_right", -2.0).unwrap();
            sink.process(1);

            assert_eq!(sink.get_output("audio_left").unwrap(), 2.0);
            assert_eq!(sink.get_output("audio_right").unwrap(), -2.0);
            let (left, right) = sink.sink_block();
            assert_eq!(left[0], 2.0);
            assert_eq!(right[0], -2.0);

            handle.finish();
        }

        #[test]
        fn configured_background_video_feeds_stream_backend() {
            let fake = fake_ffmpeg();
            let video = fake.parent().unwrap().join("loop.mp4");
            fs::write(&video, b"fake video").unwrap();
            let config = RtmpSinkConfig::from_json(
                &json!({
                    "ffmpeg_path": fake.to_string_lossy(),
                    "url": "rtmp://example.test/live/fugue",
                    "width": 2,
                    "height": 2,
                    "fps": 20,
                    "buffer_frames": 64,
                    "video_queue_frames": 2,
                    "background_video": video.to_string_lossy()
                }),
                48_000,
            )
            .unwrap();
            let (_sink, handle) = RtmpSink::new_native(config).unwrap();
            assert!(handle.has_background_video());

            // Generous headroom: the fake ffmpeg is a python3 subprocess whose
            // cold-start + socket handshake + first paced frame can exceed a
            // couple seconds on a loaded CI runner. The loop exits the instant a
            // frame arrives, so this only affects slow machines, never the happy
            // path.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while handle.stats().video_frames_sent == 0 && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }

            let stats = handle.finish();
            assert!(stats.video_frames_sent > 0, "{stats:?}");
        }
    }
}
