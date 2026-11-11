//! Backend tests, with a fake ffmpeg script and opt-in real-ffmpeg smoke tests.

use super::*;

#[test]
fn redacts_stream_urls_from_diagnostics() {
    let url = "rtmps://example.test/live/secret-stream-key";
    let error = redact_stream_url(format!("could not open {url}"), url);

    assert_eq!(error, "could not open <redacted-stream-url>");
    assert!(!error.contains("secret-stream-key"));
}

#[test]
fn missing_ffmpeg_error_is_clear() {
    let err = FfmpegStreamBackend::validate_ffmpeg("fugue-definitely-missing-ffmpeg").unwrap_err();
    assert!(err.to_string().contains("requires ffmpeg in PATH"));
}

#[cfg(unix)]
mod unix_fake_ffmpeg {
    use super::*;
    use crate::test_support::wait_until;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("fugue-rtmp-sink-{name}-{nanos}"))
    }

    fn fake_ffmpeg(mode: &str) -> PathBuf {
        let dir = temp_dir(mode);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ffmpeg");
        let script = format!(
            r#"#!/bin/sh
if [ "$1" = "-version" ]; then
  exit 0
fi
FUGUE_FAKE_FFMPEG_MODE="{mode}" python3 - "$@" <<'PY'
import os
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
    s = socket.create_connection(("127.0.0.1", port(url)), timeout=5)
    s.setblocking(False)
    sockets.append(s)

if os.environ.get("FUGUE_FAKE_FFMPEG_MODE") == "exit":
    sys.exit(2)

deadline = time.time() + 5
while sockets and time.time() < deadline:
    readable, _, _ = select.select(sockets, [], [], 0.05)
    for s in readable:
        try:
            data = s.recv(65536)
        except BlockingIOError:
            continue
        if not data:
            sockets.remove(s)
            s.close()

sys.exit(0 if not sockets else 12)
PY
"#
        );
        fs::write(&path, script).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn config_with_fake(path: &Path, url: &str) -> FfmpegStreamConfig {
        FfmpegStreamConfig {
            ffmpeg_path: path.to_string_lossy().into_owned(),
            url: url.to_string(),
            tee_to_disk: None,
            width: 2,
            height: 2,
            fps: 2,
            sample_rate: 48_000,
            video_encoder: "libx264".to_string(),
            audio_encoder: "aac".to_string(),
            video_bitrate: "2500k".to_string(),
            audio_bitrate: "128k".to_string(),
            gop_duration: 2,
            constant_video_bitrate: false,
            audio_buffer_frames: 64,
            video_queue_frames: 2,
        }
    }

    fn push_smoke_media(backend: &FfmpegStreamBackend) {
        for frame_index in 0..10 {
            let mut frame = vec![0u8; 16 * 16 * 4];
            for pixel in frame.as_chunks_mut::<4>().0 {
                pixel[0] = (frame_index * 20) as u8;
                pixel[1] = 64;
                pixel[2] = 192;
                pixel[3] = 255;
            }
            backend.push_video_rgba(&frame).unwrap();

            for sample_index in 0..4_800 {
                let phase = (sample_index as f32 / 48_000.0) * 440.0 * std::f32::consts::TAU;
                let sample = phase.sin() * 0.1;
                backend.push_audio(sample, sample);
            }
        }
    }

    #[test]
    fn streams_audio_and_video_to_fake_ffmpeg() {
        let fake = fake_ffmpeg("stream");
        FfmpegStreamBackend::validate_ffmpeg(fake.to_str().unwrap()).unwrap();
        let backend =
            FfmpegStreamBackend::start(config_with_fake(&fake, "rtmp://example.test/live"))
                .unwrap();

        backend.push_video_rgba(&[255; 16]).unwrap();
        for _ in 0..8 {
            backend.push_audio(0.25, -0.25);
        }

        backend.finish();
        let stats = backend.stats();
        assert!(stats.audio_frames_sent > 0, "{stats:?}");
        assert_eq!(stats.video_frames_sent, 1, "{stats:?}");
        assert_eq!(stats.audio_frames_dropped, 0, "{stats:?}");
        assert_eq!(stats.video_frames_dropped, 0, "{stats:?}");
    }

    #[test]
    fn rejects_bad_video_frame_size() {
        let fake = fake_ffmpeg("bad-frame");
        let backend =
            FfmpegStreamBackend::start(config_with_fake(&fake, "rtmp://example.test/live"))
                .unwrap();

        let err = backend.push_video_rgba(&[0; 15]).unwrap_err();
        assert!(err.contains("expected RGBA frame"));
        backend.finish();
    }

    #[test]
    fn restarts_after_unexpected_ffmpeg_exit() {
        let fake = fake_ffmpeg("exit");
        let backend =
            FfmpegStreamBackend::start(config_with_fake(&fake, "rtmp://example.test/exit"))
                .unwrap();

        backend.push_video_rgba(&[128; 16]).unwrap();
        for _ in 0..8 {
            backend.push_audio(0.1, 0.1);
        }

        // finish() before the first session ends would stop the worker
        // without counting a restart, so wait for the restart itself.
        wait_until(|| backend.stats().restarts > 0);

        backend.finish();
        let stats = backend.stats();
        assert!(stats.restarts > 0, "{stats:?}");
        assert!(stats.last_error.is_some(), "{stats:?}");
    }

    #[test]
    fn real_ffmpeg_writes_local_flv_when_enabled() {
        if std::env::var("FUGUE_RTMP_SINK_REAL_FFMPEG").ok().as_deref() != Some("1") {
            eprintln!("set FUGUE_RTMP_SINK_REAL_FFMPEG=1 to run real ffmpeg smoke");
            return;
        }

        FfmpegStreamBackend::validate_ffmpeg("ffmpeg").unwrap();
        let dir = temp_dir("real-ffmpeg");
        fs::create_dir_all(&dir).unwrap();
        let output = dir.join("smoke.flv");
        let backend = FfmpegStreamBackend::start(FfmpegStreamConfig {
            ffmpeg_path: "ffmpeg".to_string(),
            url: output.to_string_lossy().into_owned(),
            tee_to_disk: None,
            width: 16,
            height: 16,
            fps: 5,
            sample_rate: 48_000,
            video_encoder: "libx264".to_string(),
            audio_encoder: "aac".to_string(),
            video_bitrate: "200k".to_string(),
            audio_bitrate: "64k".to_string(),
            gop_duration: 2,
            constant_video_bitrate: false,
            audio_buffer_frames: 512,
            video_queue_frames: 8,
        })
        .unwrap();

        push_smoke_media(&backend);

        backend.finish();
        let stats = backend.stats();
        assert!(stats.audio_frames_sent > 0, "{stats:?}");
        assert!(stats.video_frames_sent > 0, "{stats:?}");
        let metadata = fs::metadata(&output).unwrap();
        assert!(metadata.len() > 0, "expected non-empty FLV at {output:?}");
    }

    #[test]
    fn real_ffmpeg_tees_stream_to_local_archive_when_enabled() {
        if std::env::var("FUGUE_RTMP_SINK_REAL_FFMPEG").ok().as_deref() != Some("1") {
            eprintln!("set FUGUE_RTMP_SINK_REAL_FFMPEG=1 to run real ffmpeg smoke");
            return;
        }

        FfmpegStreamBackend::validate_ffmpeg("ffmpeg").unwrap();
        let dir = temp_dir("real-ffmpeg-tee");
        let archive_dir = dir.join("archive captures");
        fs::create_dir_all(&archive_dir).unwrap();
        let stream_output = dir.join("stream.flv");
        let archive_output = archive_dir.join("session's [mix].mkv");
        let backend = FfmpegStreamBackend::start(FfmpegStreamConfig {
            ffmpeg_path: "ffmpeg".to_string(),
            url: stream_output.to_string_lossy().into_owned(),
            tee_to_disk: Some(archive_output.to_string_lossy().into_owned()),
            width: 16,
            height: 16,
            fps: 5,
            sample_rate: 48_000,
            video_encoder: "libx264".to_string(),
            audio_encoder: "aac".to_string(),
            video_bitrate: "200k".to_string(),
            audio_bitrate: "64k".to_string(),
            gop_duration: 2,
            constant_video_bitrate: false,
            audio_buffer_frames: 512,
            video_queue_frames: 8,
        })
        .unwrap();

        push_smoke_media(&backend);
        backend.finish();

        let stats = backend.stats();
        assert!(stats.audio_frames_sent > 0, "{stats:?}");
        assert!(stats.video_frames_sent > 0, "{stats:?}");
        assert_eq!(stats.last_error, None, "{stats:?}");
        assert!(fs::metadata(&stream_output).unwrap().len() > 0);
        assert!(fs::metadata(&archive_output).unwrap().len() > 0);
    }
}
