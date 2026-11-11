//! Backend tests over the recording fake encoder, plus opt-in real-ffmpeg
//! smoke tests.

use super::fake::{stop_when_idle, FakeEncoder, FakeSession};
use super::*;

fn fake_config(audio_buffer_frames: usize) -> FfmpegStreamConfig {
    FfmpegStreamConfig {
        ffmpeg_path: "ffmpeg".to_string(),
        url: "rtmp://example.test/live/secret-key".to_string(),
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
        audio_buffer_frames,
        video_queue_frames: 2,
    }
}

fn fake_stream(fake: &FakeEncoder, audio_buffer_frames: usize) -> FfmpegStreamHandle {
    FfmpegStreamBackend::start_on_finish(
        fake_config(audio_buffer_frames),
        Arc::new(fake.clone()),
        stop_when_idle,
    )
}

#[test]
fn production_waits_are_unchanged() {
    use std::time::Duration;
    assert_eq!(Pause::Idle.duration(), Duration::from_millis(1));
    assert_eq!(Pause::AfterExit.duration(), Duration::from_millis(100));
    assert_eq!(
        Pause::AfterStartFailure.duration(),
        Duration::from_millis(250)
    );
}

#[test]
fn streams_audio_and_video_to_the_encoder() {
    let fake = FakeEncoder::default();
    let backend = fake_stream(&fake, 64);

    let frame: Vec<u8> = (0..16).collect();
    backend.push_video_rgba(&frame).unwrap();
    for i in 0..8 {
        backend.push_audio(i as f32, -(i as f32));
    }
    backend.finish();

    let sessions = fake.sessions();
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].closed);
    assert_eq!(sessions[0].video, vec![frame]);
    let expected: Vec<_> = (0..8).map(|i| (i as f32, -(i as f32))).collect();
    assert_eq!(sessions[0].audio_frames(), expected);
    let stats = backend.stats();
    assert_eq!(stats.audio_frames_sent, 8, "{stats:?}");
    assert_eq!(stats.video_frames_sent, 1, "{stats:?}");
    assert_eq!(stats.audio_frames_dropped, 0, "{stats:?}");
    assert_eq!(stats.video_frames_dropped, 0, "{stats:?}");
    assert_eq!(stats.restarts, 0, "{stats:?}");
    assert_eq!(stats.last_error, None, "{stats:?}");
}

#[test]
fn finish_delivers_every_queued_frame_across_writes() {
    let fake = FakeEncoder::default();
    let backend = fake_stream(&fake, 8_192);
    // More than one 4096-frame write.
    let pushed: Vec<_> = (0..5_000).map(|i| (i as f32, i as f32 + 0.5)).collect();
    for &(left, right) in &pushed {
        backend.push_audio(left, right);
    }
    backend.finish();

    assert_eq!(fake.audio_frames(), pushed);
    let stats = backend.stats();
    assert_eq!(stats.audio_frames_sent, pushed.len(), "{stats:?}");
    assert_eq!(stats.audio_frames_dropped, 0, "{stats:?}");
}

#[test]
fn full_audio_ring_drops_newest_frames() {
    let fake = FakeEncoder::default();
    // A ring of four slots holds three frames.
    let backend = fake_stream(&fake, 4);
    for i in 0..5 {
        backend.push_audio(i as f32, 0.0);
    }
    backend.finish();

    let left: Vec<_> = fake.audio_frames().iter().map(|frame| frame.0).collect();
    assert_eq!(left, [0.0, 1.0, 2.0]);
    let stats = backend.stats();
    assert_eq!(stats.audio_frames_sent, 3, "{stats:?}");
    assert_eq!(stats.audio_frames_dropped, 2, "{stats:?}");
}

#[test]
fn rejects_bad_video_frame_size() {
    let fake = FakeEncoder::default();
    let backend = fake_stream(&fake, 64);

    let err = backend.push_video_rgba(&[0; 15]).unwrap_err();
    assert!(err.contains("expected RGBA frame with 16 bytes"), "{err}");
    backend.finish();
    assert!(fake
        .sessions()
        .iter()
        .all(|session| session.video.is_empty()));
}

#[test]
fn restarts_after_unexpected_encoder_exit() {
    let fake = FakeEncoder::scripted([FakeSession::ExitAfterWrites(
        1,
        "ffmpeg exited unexpectedly: exit status: 2".to_string(),
    )]);
    let backend = FfmpegStreamBackend::start_on_finish(
        fake_config(64),
        Arc::new(fake.clone()),
        |backend, pause| match pause {
            Pause::AfterExit => {
                for i in 4..8 {
                    backend.push_audio(i as f32, 0.5);
                }
            }
            Pause::Idle => backend.stop(),
            Pause::AfterStartFailure => panic!("no start should fail"),
        },
    );
    for i in 0..4 {
        backend.push_audio(i as f32, 0.5);
    }
    backend.run_worker();

    let sessions = fake.sessions();
    assert_eq!(sessions.len(), 2);
    let frames =
        |range: std::ops::Range<i32>| -> Vec<_> { range.map(|i| (i as f32, 0.5)).collect() };
    assert_eq!(sessions[0].audio_frames(), frames(0..4));
    assert_eq!(sessions[1].audio_frames(), frames(4..8));
    let stats = backend.stats();
    assert_eq!(stats.audio_frames_sent, 8, "{stats:?}");
    assert_eq!(stats.restarts, 1, "{stats:?}");
    assert_eq!(
        stats.last_error.as_deref(),
        Some("ffmpeg exited unexpectedly: exit status: 2")
    );
}

#[test]
fn restarts_after_failed_start() {
    let fake = FakeEncoder::scripted([FakeSession::FailToStart(
        "failed to spawn ffmpeg 'ffmpeg': not found".to_string(),
    )]);
    let backend = fake_stream(&fake, 64);
    backend.run_worker();

    assert_eq!(fake.starts(), 2);
    let stats = backend.stats();
    assert_eq!(stats.restarts, 1, "{stats:?}");
    let error = stats.last_error.unwrap();
    assert!(error.contains("failed to spawn ffmpeg"), "{error}");
}

#[test]
fn restarts_after_failed_write() {
    let fake = FakeEncoder::scripted([FakeSession::FailWriteAfter(0)]);
    let backend = fake_stream(&fake, 64);
    backend.push_audio(0.25, -0.25);
    backend.run_worker();

    let sessions = fake.sessions();
    assert_eq!(sessions.len(), 2);
    assert!(sessions.iter().all(|session| session.closed));
    // The failed write's frame is lost with the session.
    assert!(sessions[1].audio.is_empty());
    let stats = backend.stats();
    assert_eq!(stats.restarts, 1, "{stats:?}");
    let error = stats.last_error.unwrap();
    assert!(error.starts_with("ffmpeg audio write failed"), "{error}");
}

#[test]
fn exit_while_stopping_ends_without_restart() {
    let fake = FakeEncoder::scripted([FakeSession::ExitOnFirstCheck(
        "ffmpeg exited unexpectedly: rtmp://example.test/live/secret-key".to_string(),
    )]);
    let backend = fake_stream(&fake, 64);
    backend.push_audio(0.1, 0.1);
    backend.finish();

    assert_eq!(fake.starts(), 1);
    let stats = backend.stats();
    assert_eq!(stats.restarts, 0, "{stats:?}");
    assert_eq!(
        stats.last_error.as_deref(),
        Some("ffmpeg exited unexpectedly: <redacted-stream-url>")
    );
}

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

/// Opt-in: set `FUGUE_RTMP_SINK_REAL_FFMPEG=1` to stream through real ffmpeg.
#[cfg(unix)]
mod real_ffmpeg {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("fugue-rtmp-sink-{name}-{nanos}"))
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
