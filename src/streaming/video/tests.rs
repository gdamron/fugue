use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::decoder::FfmpegVideoCommandSpec;
use super::fake::{FakeDecoders, FakeRun, SteppedClock};
use super::*;
use crate::test_support::wait_until;

/// Bytes in one fake 2x2 RGBA frame.
const FRAME: usize = 16;
const FAKE_FPS: u32 = 20;

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

fn period(fps: u32) -> Duration {
    Duration::from_secs_f64(1.0 / fps as f64)
}

/// Frames a target received, each with the clock time it arrived.
type Received = Arc<Mutex<Vec<(Duration, Vec<u8>)>>>;

fn recording_target(clock: &Arc<SteppedClock>) -> (VideoFrameTarget, Received) {
    let received = Received::default();
    let target_received = received.clone();
    let clock = clock.clone();
    let target: VideoFrameTarget = Arc::new(move |frame| {
        target_received
            .lock()
            .unwrap()
            .push((clock.elapsed(), frame.to_vec()));
        Ok(())
    });
    (target, received)
}

fn fake_config(autoplay: bool, loop_enabled: bool) -> VideoPlaybackConfig {
    VideoPlaybackConfig {
        width: 2,
        height: 2,
        fps: FAKE_FPS,
        autoplay,
        loop_enabled,
        // Any existing file: the fake decoder never opens it.
        ..config(PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/Cargo.toml"
        )))
    }
}

struct Rig {
    clock: Arc<SteppedClock>,
    decoders: Arc<FakeDecoders>,
    received: Received,
    playback: VideoPlaybackHandle,
}

impl Rig {
    /// Fake 2x2 playback at 20 fps of `runs`, settled after starting.
    fn start(runs: Vec<FakeRun>, autoplay: bool, loop_enabled: bool) -> Self {
        let clock = SteppedClock::new();
        let decoders = FakeDecoders::new(&clock, runs);
        let (target, received) = recording_target(&clock);
        let config = fake_config(autoplay, loop_enabled);
        let playback = VideoPlayback::start_with(config, target, clock.runtime(&decoders)).unwrap();
        clock.settle();
        Self {
            clock,
            decoders,
            received,
            playback,
        }
    }

    fn step(&self) {
        self.clock.advance(period(FAKE_FPS));
    }

    /// The first byte of each received frame, which names it.
    fn frames(&self) -> Vec<u8> {
        self.received
            .lock()
            .unwrap()
            .iter()
            .map(|(_, frame)| frame[0])
            .collect()
    }

    fn times(&self) -> Vec<Duration> {
        self.received
            .lock()
            .unwrap()
            .iter()
            .map(|(at, _)| *at)
            .collect()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.playback.finish();
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
    let err = VideoPlayback::start_with(
        config(PathBuf::from("definitely-missing.mp4")),
        target,
        VideoRuntime::system(),
    )
    .err()
    .expect("missing file should fail");
    assert!(err.to_string().contains("could not open background video"));
}

#[test]
fn black_fallback_emits_paced_frames_until_external_video_takes_over() {
    let clock = SteppedClock::new();
    let (target, received) = recording_target(&clock);
    let fallback = BlackVideoFallback::start_with_clock(2, 2, 100, target, clock.clone()).unwrap();
    let period = period(100);
    let times = || -> Vec<Duration> { received.lock().unwrap().iter().map(|r| r.0).collect() };

    clock.settle();
    assert_eq!(times(), [Duration::ZERO]);
    clock.advance(period - Duration::from_nanos(1));
    assert_eq!(times().len(), 1, "no frame before its period is up");
    clock.advance(Duration::from_nanos(1));
    clock.advance(period);
    assert_eq!(times(), [Duration::ZERO, period, period * 2]);
    assert!(received.lock().unwrap().iter().all(|r| r.1 == [0; FRAME]));

    fallback.external_video_started();
    clock.advance(period * 5);
    assert_eq!(times().len(), 3, "external video replaces the fallback");
    fallback.finish();
    assert_eq!(fallback.last_error(), None);
}

#[test]
fn black_fallback_reports_a_failed_delivery() {
    let clock = SteppedClock::new();
    let target: VideoFrameTarget = Arc::new(|_| Err("queue closed".to_string()));
    let fallback = BlackVideoFallback::start_with_clock(2, 2, 30, target, clock.clone()).unwrap();
    clock.settle();
    fallback.finish();
    assert_eq!(
        fallback.last_error().as_deref(),
        Some("could not deliver black fallback video frame: queue closed")
    );
}

#[test]
#[should_panic(expected = "the video worker exited")]
fn a_worker_that_panics_fails_the_advance_instead_of_hanging() {
    let clock = SteppedClock::new();
    let target: VideoFrameTarget = Arc::new(|_| panic!("frame target broke"));
    let _fallback = BlackVideoFallback::start_with_clock(2, 2, 30, target, clock.clone()).unwrap();
    clock.settle();
}

#[test]
fn emits_decoded_frames_in_order_on_the_media_clock() {
    let rig = Rig::start(vec![FakeRun::frames(3, FRAME)], true, false);
    let period = period(FAKE_FPS);

    assert_eq!(rig.frames(), [1]);
    rig.clock.advance(period - Duration::from_nanos(1));
    assert_eq!(rig.frames(), [1], "no frame before its period is up");
    rig.clock.advance(Duration::from_nanos(1));
    assert_eq!(rig.frames(), [1, 2]);
    rig.step();
    rig.step();
    assert_eq!(rig.frames(), [1, 2, 3], "playback ends without looping");
    assert_eq!(rig.times(), [Duration::ZERO, period, period * 2]);
    let stats = rig.playback.stats();
    assert_eq!((stats.frames_emitted, stats.loops), (3, 0));
    assert_eq!(stats.last_error, None);
    assert_eq!(rig.received.lock().unwrap()[1].1, [2; FRAME]);
}

#[test]
fn loops_with_a_fresh_decoder_on_the_same_media_clock() {
    let rig = Rig::start(vec![FakeRun::frames(2, FRAME)], true, true);
    for _ in 0..3 {
        rig.step();
    }
    assert_eq!(rig.frames(), [1, 2, 1, 2]);
    let period = period(FAKE_FPS);
    assert_eq!(rig.times(), [0, 1, 2, 3].map(|n| period * n));
    // The second loop's decoder is already waiting on its first frame.
    assert_eq!(rig.decoders.start_attempts(), 3);
    assert_eq!(rig.playback.stats().loops, 2);
}

#[test]
fn waits_for_play_and_holds_while_paused() {
    let rig = Rig::start(vec![FakeRun::frames(5, FRAME)], false, false);
    rig.clock.advance(Duration::from_secs(1));
    assert_eq!(rig.frames(), [] as [u8; 0]);
    assert_eq!(rig.decoders.start_attempts(), 0);

    rig.playback.play();
    rig.clock.settle();
    rig.step();
    assert_eq!(rig.frames(), [1, 2]);

    rig.playback.pause();
    rig.clock.advance(Duration::from_secs(1));
    assert_eq!(rig.frames(), [1, 2], "no frames while paused");

    // Resuming plays the next frame now, not the frames the pause skipped.
    rig.playback.play();
    rig.clock.settle();
    rig.step();
    assert_eq!(rig.frames(), [1, 2, 3, 4]);
    let resumed_at = rig.times()[2];
    assert_eq!(rig.times()[3], resumed_at + period(FAKE_FPS));
}

#[test]
fn restart_replaces_the_running_decoder_and_plays_from_the_start() {
    let rig = Rig::start(vec![FakeRun::frames(2, FRAME).hold_open()], false, true);
    rig.playback.play();
    rig.clock.settle();
    rig.step();
    assert_eq!(rig.frames(), [1, 2]);
    rig.playback.pause();
    rig.clock.settle();

    // Restart also resumes a paused playback.
    rig.playback.set_loop_enabled(false);
    rig.playback.restart();
    rig.clock.settle();
    assert!(rig.decoders.killed(0));
    assert!(!rig.decoders.killed(1));
    rig.step();
    assert_eq!(rig.frames(), [1, 2, 1, 2]);
    assert_eq!(rig.decoders.start_attempts(), 2);
}

#[test]
fn restart_kills_only_a_decoder_started_for_an_older_generation() {
    let rig = Rig::start(vec![FakeRun::frames(1, FRAME).hold_open()], false, false);
    // Stop the worker so the decoder slot is ours alone.
    rig.playback.finish();
    let shared = &rig.playback.shared;
    let running = |generation| {
        let launched = rig.decoders.launch(&fake_config(false, false)).unwrap();
        *shared.child.lock().unwrap() = Some((generation, launched.process));
    };

    // A restart to generation 1 finds the worker already running
    // generation 1's decoder: that decoder keeps running.
    running(1);
    shared.kill_stale_decoder(1);
    assert!(!rig.decoders.killed(0));
    shared.kill_decoder();
    assert!(rig.decoders.killed(0));

    // An older generation's decoder is killed.
    running(0);
    shared.kill_stale_decoder(1);
    assert!(rig.decoders.killed(1));
}

#[test]
fn a_decoder_that_cannot_start_pauses_with_an_error_until_played() {
    let rig = Rig::start(
        vec![
            FakeRun::fail_to_start("could not start background video decoder: not found"),
            FakeRun::frames(1, FRAME),
        ],
        true,
        true,
    );
    assert_eq!(rig.frames(), [] as [u8; 0]);
    assert_eq!(
        rig.playback.stats().last_error.as_deref(),
        Some("could not start background video decoder: not found")
    );
    rig.clock.advance(Duration::from_secs(1));
    assert_eq!(rig.decoders.start_attempts(), 1, "no retry until played");

    rig.playback.play();
    rig.clock.settle();
    assert_eq!(rig.frames(), [1]);
}

#[test]
fn a_failed_decoder_exit_stops_playback_with_its_status() {
    let rig = Rig::start(
        vec![FakeRun::frames(1, FRAME).exit_failure("exit status: 1")],
        true,
        true,
    );
    rig.step();
    rig.step();
    assert_eq!(rig.frames(), [1], "a failed decode does not loop");
    let stats = rig.playback.stats();
    assert_eq!(stats.loops, 0);
    assert_eq!(
        stats.last_error.as_deref(),
        Some("background video decoder exited with status: exit status: 1")
    );
}

#[test]
fn a_looping_video_with_no_frames_pauses_with_an_error_instead_of_relaunching() {
    // A relaunch would reach the second run, which fails to start and
    // pauses, so the old relaunch loop fails this test instead of hanging.
    let rig = Rig::start(
        vec![
            FakeRun::bytes(Vec::new()),
            FakeRun::fail_to_start("the decoder was relaunched"),
        ],
        true,
        true,
    );
    rig.clock.advance(Duration::from_secs(1));
    assert_eq!(rig.decoders.start_attempts(), 1, "no relaunch until played");
    let stats = rig.playback.stats();
    assert_eq!(stats.loops, 0);
    assert_eq!(
        stats.last_error.as_deref(),
        Some("background video decoder produced no frames")
    );
}

#[test]
fn a_looping_video_that_stops_producing_frames_pauses_after_its_last_frame() {
    let rig = Rig::start(
        vec![
            FakeRun::frames(1, FRAME),
            FakeRun::bytes(Vec::new()),
            FakeRun::fail_to_start("the decoder was relaunched"),
        ],
        true,
        true,
    );
    // The loop pass starts at once, so settling the start runs both passes.
    assert_eq!(rig.frames(), [1]);
    assert_eq!(rig.decoders.start_attempts(), 2);
    let stats = rig.playback.stats();
    assert_eq!(stats.loops, 1);
    assert_eq!(
        stats.last_error.as_deref(),
        Some("background video decoder produced no frames")
    );
}

#[test]
fn a_video_with_no_frames_reports_why_it_stopped() {
    let rig = Rig::start(vec![FakeRun::bytes(Vec::new())], true, false);
    rig.step();
    assert_eq!(rig.decoders.start_attempts(), 1);
    assert_eq!(
        rig.playback.stats().last_error.as_deref(),
        Some("background video decoder produced no frames")
    );
}

#[test]
fn an_incomplete_frame_stops_playback_with_an_error() {
    let mut output = vec![7; FRAME];
    output.extend([8; 4]);
    let rig = Rig::start(vec![FakeRun::bytes(output)], true, true);
    rig.step();
    assert_eq!(rig.frames(), [7]);
    let error = rig.playback.stats().last_error.unwrap();
    assert!(
        error.starts_with("background video decoder produced an incomplete frame"),
        "{error}"
    );
    assert_eq!(rig.decoders.start_attempts(), 1);
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
    let playback = VideoPlayback::start_with(config, target, VideoRuntime::system()).unwrap();
    wait_until(|| playback.stats().frames_emitted >= 3);
    playback.finish();

    assert!(frames.load(Ordering::Acquire) >= 3);
    assert_eq!(playback.stats().last_error, None);
}
