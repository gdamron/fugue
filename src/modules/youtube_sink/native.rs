//! YouTube-specific configuration mapped onto the shared RTMP sink.

use serde_json::{Map, Value};

use crate::modules::rtmp_sink::{RtmpSink, RtmpSinkConfig, RtmpSinkHandle};
use crate::streaming::ffmpeg::FfmpegStreamBackend;

const DEFAULT_SERVER_URL: &str = "rtmps://a.rtmps.youtube.com:443/live2";
const DEFAULT_STREAM_KEY_ENV: &str = "YOUTUBE_STREAM_KEY";

pub(super) fn build(
    config: &Value,
    sample_rate: u32,
) -> Result<(RtmpSink, RtmpSinkHandle), Box<dyn std::error::Error>> {
    let config = from_json_with_env(config, sample_rate, |name| std::env::var(name).ok())?;
    FfmpegStreamBackend::validate_ffmpeg(&config.ffmpeg_path)?;
    RtmpSink::new_native_with_options(config, true, true)
}

fn from_json_with_env(
    config: &Value,
    sample_rate: u32,
    env: impl Fn(&str) -> Option<String>,
) -> Result<RtmpSinkConfig, Box<dyn std::error::Error>> {
    let mut mapped = match config {
        Value::Null => Map::new(),
        Value::Object(values) => values.clone(),
        _ => return Err("youtube_sink config must be an object".into()),
    };

    if mapped.contains_key("url") {
        return Err("youtube_sink config.url is unsupported; use config.server_url".into());
    }

    let stream_key_env = optional_nonempty_string(&mapped, "stream_key_env")?;
    let inline_stream_key = optional_nonempty_string(&mapped, "stream_key")?;
    let stream_key = if let Some(name) = stream_key_env {
        env(&name)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                format!("youtube_sink stream key environment variable {name} is missing or empty")
            })?
    } else if let Some(stream_key) = inline_stream_key {
        stream_key
    } else {
        env(DEFAULT_STREAM_KEY_ENV)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                format!(
                    "youtube_sink requires config.stream_key or environment variable {DEFAULT_STREAM_KEY_ENV}"
                )
            })?
    };

    let server_url = optional_nonempty_string(&mapped, "server_url")?
        .unwrap_or_else(|| DEFAULT_SERVER_URL.to_string());
    let server_url = server_url.trim_end_matches('/');
    if !server_url.starts_with("rtmps://") || server_url.len() == "rtmps://".len() {
        return Err("youtube_sink config.server_url must be a valid rtmps:// URL".into());
    }
    if server_url.chars().any(char::is_whitespace) {
        return Err("youtube_sink config.server_url must not contain whitespace".into());
    }

    mapped.remove("stream_key_env");
    mapped.remove("stream_key");
    mapped.remove("server_url");
    mapped.insert(
        "url".to_string(),
        Value::String(format!("{server_url}/{}", stream_key.trim())),
    );
    if !mapped.contains_key("width")
        && !mapped.contains_key("height")
        && !mapped.contains_key("resolution")
    {
        mapped.insert(
            "resolution".to_string(),
            Value::String("1920x1080".to_string()),
        );
    }
    mapped
        .entry("fps".to_string())
        .or_insert_with(|| Value::from(30));
    mapped
        .entry("video_encoder".to_string())
        .or_insert_with(|| Value::String("libx264".to_string()));
    mapped
        .entry("audio_encoder".to_string())
        .or_insert_with(|| Value::String("aac".to_string()));
    mapped
        .entry("video_bitrate".to_string())
        .or_insert_with(|| Value::from(10_000));
    mapped
        .entry("audio_bitrate".to_string())
        .or_insert_with(|| Value::from(128));
    mapped
        .entry("gop_seconds".to_string())
        .or_insert_with(|| Value::from(2));

    RtmpSinkConfig::from_json(&Value::Object(mapped), sample_rate)
}

fn optional_nonempty_string(
    config: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    match config.get(key) {
        Some(Value::String(value)) if !value.trim().is_empty() => {
            Ok(Some(value.trim().to_string()))
        }
        Some(Value::String(_)) => {
            Err(format!("youtube_sink config.{key} must not be empty").into())
        }
        Some(_) => Err(format!("youtube_sink config.{key} must be a string").into()),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn applies_youtube_defaults_from_default_environment_variable() {
        let config = from_json_with_env(&json!({}), 48_000, |name| {
            (name == DEFAULT_STREAM_KEY_ENV).then(|| "default-key".to_string())
        })
        .unwrap();

        assert_eq!(
            config.url,
            "rtmps://a.rtmps.youtube.com:443/live2/default-key"
        );
        assert_eq!((config.width, config.height), (1920, 1080));
        assert_eq!(config.fps, 30);
        assert_eq!(config.video_encoder, "libx264");
        assert_eq!(config.audio_encoder, "aac");
        assert_eq!(config.video_bitrate, "10000k");
        assert_eq!(config.audio_bitrate, "128k");
        assert_eq!(config.gop_seconds, 2);
    }

    #[test]
    fn explicit_environment_source_wins_over_inline_key() {
        let config = from_json_with_env(
            &json!({
                "stream_key_env": "CUSTOM_YOUTUBE_KEY",
                "stream_key": "inline-key"
            }),
            48_000,
            |name| (name == "CUSTOM_YOUTUBE_KEY").then(|| "environment-key".to_string()),
        )
        .unwrap();

        assert!(config.url.ends_with("/environment-key"));
        assert!(!config.url.contains("inline-key"));
    }

    #[test]
    fn inline_key_and_encoding_overrides_are_supported() {
        let config = from_json_with_env(
            &json!({
                "stream_key": "inline-key",
                "server_url": "rtmps://backup.example.test:443/live/",
                "resolution": "1280x720",
                "fps": 60,
                "video_bitrate": 6000,
                "background_video": "./loop.mp4"
            }),
            44_100,
            |_| None,
        )
        .unwrap();

        assert_eq!(
            config.url,
            "rtmps://backup.example.test:443/live/inline-key"
        );
        assert_eq!((config.width, config.height), (1280, 720));
        assert_eq!(config.fps, 60);
        assert_eq!(config.video_bitrate, "6000k");
        assert_eq!(config.sample_rate, 44_100);
        assert_eq!(config.background_video.as_deref(), Some("./loop.mp4"));
    }

    #[test]
    fn rejects_missing_keys_and_insecure_server_urls_without_echoing_secrets() {
        let missing = from_json_with_env(&json!({}), 48_000, |_| None)
            .unwrap_err()
            .to_string();
        assert!(missing.contains(DEFAULT_STREAM_KEY_ENV));

        let insecure = from_json_with_env(
            &json!({
                "stream_key": "do-not-echo-this",
                "server_url": "rtmp://example.test/live"
            }),
            48_000,
            |_| None,
        )
        .unwrap_err()
        .to_string();
        assert!(insecure.contains("rtmps://"));
        assert!(!insecure.contains("do-not-echo-this"));
    }

    #[test]
    fn rejects_missing_explicit_environment_even_with_inline_fallback() {
        let error = from_json_with_env(
            &json!({
                "stream_key_env": "MISSING_KEY",
                "stream_key": "inline-key"
            }),
            48_000,
            |_| None,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("MISSING_KEY"));
        assert!(!error.contains("inline-key"));
    }

    #[cfg(unix)]
    mod unix_streaming {
        use super::*;
        use crate::{Module, MAX_BLOCK};
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::path::PathBuf;
        use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

        fn fake_ffmpeg() -> PathBuf {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!("fugue-youtube-sink-{nanos}"));
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
    sys.exit(10)

def port(url):
    match = re.search(r":(\d+)(?:\?|$)", url)
    if not match:
        sys.exit(11)
    return int(match.group(1))

sockets = []
for url in urls[:2]:
    stream = socket.create_connection(("127.0.0.1", port(url)), timeout=5)
    stream.setblocking(False)
    sockets.append(stream)

deadline = time.time() + 5
while sockets and time.time() < deadline:
    readable, _, _ = select.select(sockets, [], [], 0.05)
    for stream in readable:
        data = stream.recv(65536)
        if not data:
            sockets.remove(stream)
            stream.close()

sys.exit(0 if not sockets else 12)
PY
"#;
            fs::write(&path, script).unwrap();
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).unwrap();
            path
        }

        #[test]
        fn streams_black_video_without_changing_audio_pass_through() {
            let ffmpeg = fake_ffmpeg();
            let config = from_json_with_env(
                &json!({
                    "stream_key": "fake-key",
                    "server_url": "rtmps://example.test:443/live",
                    "ffmpeg_path": ffmpeg.to_string_lossy(),
                    "width": 2,
                    "height": 2,
                    "fps": 50,
                    "buffer_frames": 64,
                    "video_queue_frames": 2,
                    "monitor": true
                }),
                48_000,
                |_| None,
            )
            .unwrap();
            FfmpegStreamBackend::validate_ffmpeg(&config.ffmpeg_path).unwrap();
            let (mut sink, handle) = RtmpSink::new_native_with_options(config, true, true).unwrap();

            sink.set_input("audio_left", 0.25).unwrap();
            sink.set_input("audio_right", -0.5).unwrap();
            sink.process(1);
            assert_eq!(sink.get_output("audio_left").unwrap(), 0.25);
            assert_eq!(sink.get_output("audio_right").unwrap(), -0.5);

            let deadline = Instant::now() + Duration::from_secs(2);
            while handle.stats().video_frames_sent == 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            let stats = handle.finish();
            assert!(stats.audio_frames_sent > 0, "{stats:?}");
            assert!(stats.video_frames_sent > 0, "{stats:?}");
            assert_eq!(stats.last_error, None, "{stats:?}");
        }

        #[test]
        #[ignore = "requires an operator-created YouTube broadcast and explicit opt-in"]
        fn real_youtube_ingestion_smoke() {
            if std::env::var("FUGUE_YOUTUBE_SINK_REAL_FFMPEG")
                .ok()
                .as_deref()
                != Some("1")
            {
                eprintln!("set FUGUE_YOUTUBE_SINK_REAL_FFMPEG=1 to enable this smoke test");
                return;
            }

            let config =
                from_json_with_env(&json!({}), 48_000, |name| std::env::var(name).ok()).unwrap();
            FfmpegStreamBackend::validate_ffmpeg(&config.ffmpeg_path).unwrap();
            let (mut sink, handle) = RtmpSink::new_native_with_options(config, true, true).unwrap();
            let smoke_seconds = std::env::var("FUGUE_YOUTUBE_SMOKE_SECONDS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(15);
            let block_frames = 480;
            let block_duration = Duration::from_millis(10);
            let blocks = smoke_seconds * 100;
            let mut phase = 0.0_f32;
            let phase_step = 440.0 * std::f32::consts::TAU / 48_000.0;

            for _ in 0..blocks {
                let mut samples = [0.0_f32; MAX_BLOCK];
                for sample in &mut samples[..block_frames] {
                    *sample = phase.sin() * 0.05;
                    phase = (phase + phase_step) % std::f32::consts::TAU;
                }
                sink.input_block_mut(1)[..block_frames].copy_from_slice(&samples[..block_frames]);
                sink.input_block_mut(2)[..block_frames].copy_from_slice(&samples[..block_frames]);
                sink.process(block_frames);
                std::thread::sleep(block_duration);
            }

            let stats = handle.finish();
            assert!(stats.audio_frames_sent > 0, "{stats:?}");
            assert!(stats.video_frames_sent > 0, "{stats:?}");
            assert_eq!(stats.last_error, None, "{stats:?}");
        }
    }
}
