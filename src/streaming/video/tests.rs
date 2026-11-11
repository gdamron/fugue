use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::decoder::FfmpegVideoCommandSpec;
use super::*;
use crate::test_support::wait_until;

fn config(path: PathBuf) -> VideoPlaybackConfig {
    VideoPlaybackConfig {
        ffmpeg_path: "ffmpeg".to_string(),
        path,
        width: 640,
        height: 360,
        fps: 30,
        autoplay: true,
        loop_enabled: true,
    }
}

#[test]
fn builds_picture_only_rgba_decode_command() {
    let config = config(PathBuf::from("loop with spaces.mp4"));
    let spec = FfmpegVideoCommandSpec::new(&config);
    let args = spec.args.join(" ");

    assert_eq!(spec.program, "ffmpeg");
    assert!(args.contains("-i loop with spaces.mp4 -map 0:v:0 -an"));
    assert!(args.contains("scale=640:360:force_original_aspect_ratio=decrease"));
    assert!(args.contains("pad=640:360:(ow-iw)/2:(oh-ih)/2:color=black,fps=30"));
    assert!(args.ends_with("-pix_fmt rgba -f rawvideo pipe:1"));
}

#[test]
fn rejects_missing_background_video_before_starting_worker() {
    let target: VideoFrameTarget = Arc::new(|_| Ok(()));
    let err = VideoPlayback::start(config(PathBuf::from("definitely-missing.mp4")), target)
        .err()
        .expect("missing file should fail");
    assert!(err.to_string().contains("could not open background video"));
}

#[test]
fn black_fallback_emits_paced_frames_until_external_video_takes_over() {
    let frames = Arc::new(AtomicUsize::new(0));
    let target_frames = frames.clone();
    let target: VideoFrameTarget = Arc::new(move |frame| {
        assert_eq!(frame, &[0; 16]);
        target_frames.fetch_add(1, Ordering::Relaxed);
        Ok(())
    });
    let fallback = BlackVideoFallback::start(2, 2, 100, target).unwrap();

    assert!(wait_until(|| frames.load(Ordering::Acquire) >= 3));

    fallback.external_video_started();
    // Read after the takeover so frames emitted while this thread was
    // descheduled are not miscounted; at most one frame already past the
    // worker's takeover check can still land.
    let before_takeover = frames.load(Ordering::Acquire);
    thread::sleep(Duration::from_millis(50));
    fallback.finish();
    assert!(frames.load(Ordering::Acquire) <= before_takeover + 1);
    assert_eq!(fallback.last_error(), None);
}

#[test]
fn real_ffmpeg_decodes_local_video_when_enabled() {
    if std::env::var("FUGUE_BACKGROUND_VIDEO_REAL_FFMPEG")
        .ok()
        .as_deref()
        != Some("1")
    {
        eprintln!("set FUGUE_BACKGROUND_VIDEO_REAL_FFMPEG=1 to run real ffmpeg playback smoke");
        return;
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("fugue-video-real-{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    let video = dir.join("loop.mp4");
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=0x3060c0:s=16x16:r=5:d=0.6",
            "-an",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-y",
        ])
        .arg(&video)
        .status()
        .unwrap();
    assert!(status.success());

    let frames = Arc::new(AtomicUsize::new(0));
    let target_frames = frames.clone();
    let target: VideoFrameTarget = Arc::new(move |frame| {
        assert_eq!(frame.len(), 16 * 16 * 4);
        target_frames.fetch_add(1, Ordering::Relaxed);
        Ok(())
    });
    let mut config = config(video);
    config.width = 16;
    config.height = 16;
    config.fps = 5;
    config.loop_enabled = false;
    let playback = VideoPlayback::start(config, target).unwrap();
    wait_until(|| playback.stats().frames_emitted >= 3);
    playback.finish();

    assert!(frames.load(Ordering::Acquire) >= 3);
    assert_eq!(playback.stats().last_error, None);
}

#[cfg(unix)]
mod unix_fake_ffmpeg {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture(frame_count: usize) -> (PathBuf, PathBuf) {
        // The wall clock is only microsecond-resolution on macOS, so tests
        // starting in parallel could share a directory and run each
        // other's fake decoder. The counter keeps each fixture distinct.
        static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);
        let fixture_id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "fugue-video-playback-{}-{nanos}-{fixture_id}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let ffmpeg = dir.join("ffmpeg");
        let video = dir.join("loop.mp4");
        fs::write(&video, b"fake video").unwrap();
        let script = format!(
            r#"#!/bin/sh
python3 - <<'PY'
import sys
for value in range(1, {frame_count} + 1):
    sys.stdout.buffer.write(bytes([value]) * 16)
sys.stdout.flush()
PY
"#
        );
        fs::write(&ffmpeg, script).unwrap();
        let mut permissions = fs::metadata(&ffmpeg).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&ffmpeg, permissions).unwrap();
        (ffmpeg, video)
    }

    fn fake_config(ffmpeg: PathBuf, video: PathBuf) -> VideoPlaybackConfig {
        VideoPlaybackConfig {
            ffmpeg_path: ffmpeg.to_string_lossy().into_owned(),
            path: video,
            width: 2,
            height: 2,
            fps: 20,
            autoplay: true,
            loop_enabled: false,
        }
    }

    #[test]
    fn emits_frames_on_media_clock() {
        let (ffmpeg, video) = fixture(3);
        let received = Arc::new(Mutex::new(Vec::new()));
        let target_received = received.clone();
        let target: VideoFrameTarget = Arc::new(move |frame| {
            target_received
                .lock()
                .unwrap()
                .push((Instant::now(), frame.to_vec()));
            Ok(())
        });
        let playback = VideoPlayback::start(fake_config(ffmpeg, video), target).unwrap();

        wait_until(|| playback.stats().frames_emitted >= 3);
        playback.finish();

        let received = received.lock().unwrap();
        assert_eq!(received.len(), 3);
        assert_eq!(received[0].1, vec![1; 16]);
        assert_eq!(received[2].1, vec![3; 16]);
        assert!(received[2].0.duration_since(received[0].0) >= Duration::from_millis(80));
    }

    #[test]
    fn supports_pause_restart_and_loop_control() {
        let (ffmpeg, video) = fixture(1);
        let frames = Arc::new(AtomicUsize::new(0));
        let target_frames = frames.clone();
        let target: VideoFrameTarget = Arc::new(move |_| {
            target_frames.fetch_add(1, Ordering::Relaxed);
            Ok(())
        });
        let mut config = fake_config(ffmpeg, video);
        config.autoplay = false;
        config.loop_enabled = true;
        let playback = VideoPlayback::start(config, target).unwrap();

        thread::sleep(Duration::from_millis(75));
        assert_eq!(frames.load(Ordering::Acquire), 0);

        playback.play();
        // Each loop spawns a fresh decoder, so two frames prove a loop.
        assert!(wait_until(|| frames.load(Ordering::Acquire) >= 2));
        assert!(playback.stats().loops > 0);

        playback.pause();
        // A frame already past its delivery wait when pause() landed may
        // still be emitted, however late the worker is scheduled; any
        // further frame means pause was ignored.
        let at_pause = frames.load(Ordering::Acquire);
        thread::sleep(Duration::from_millis(175));
        let paused_at = frames.load(Ordering::Acquire);
        assert!(paused_at <= at_pause + 1, "{paused_at} > {at_pause} + 1");

        playback.set_loop_enabled(false);
        playback.restart();
        assert!(wait_until(|| frames.load(Ordering::Acquire) > paused_at));
        playback.finish();
        assert!(frames.load(Ordering::Acquire) > paused_at);
    }

    #[test]
    fn restart_kills_only_a_decoder_started_for_an_older_generation() {
        let (ffmpeg, video) = fixture(1);
        let mut config = fake_config(ffmpeg, video);
        config.autoplay = false;
        let target: VideoFrameTarget = Arc::new(|_| Ok(()));
        let playback = VideoPlayback::start(config, target).unwrap();
        // Stop the worker so the decoder slot is ours alone.
        playback.finish();

        let running = |generation| {
            let child = Command::new("sleep").arg("30").spawn().unwrap();
            *playback.shared.child.lock().unwrap() = Some((generation, child));
        };
        let exited = || {
            let mut slot = playback.shared.child.lock().unwrap();
            let (_, child) = slot.as_mut().unwrap();
            wait_until(|| child.try_wait().unwrap().is_some())
        };

        // A restart to generation 1 finds the worker already running
        // generation 1's decoder: that decoder keeps running.
        running(1);
        playback.shared.kill_stale_decoder(1);
        assert!(playback
            .shared
            .child
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .1
            .try_wait()
            .unwrap()
            .is_none());
        playback.shared.kill_decoder();
        assert!(exited());

        // An older generation's decoder is killed.
        running(0);
        playback.shared.kill_stale_decoder(1);
        assert!(exited());
    }
}
