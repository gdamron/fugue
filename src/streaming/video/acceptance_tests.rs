//! Acceptance tests for the frames a stream's picture is made of: decoded
//! background video, or black frames when there is none. Both are paced at
//! the configured fps.
//!
//! No ffmpeg, no subprocess and no sleeping: a stepped test clock supplies
//! time, and `advance`/`settle` return once the video worker has caught up.
//! The worker level records what reaches the frame target; the sink level
//! checks what reaches the stream's encoder session.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use super::fake::{FakeDecoders, FakeRun, SteppedClock};
use super::{
    BlackVideoFallback, BlackVideoFallbackHandle, VideoFrameTarget, VideoPlayback,
    VideoPlaybackConfig, VideoPlaybackHandle,
};
use crate::modules::rtmp_sink::{RtmpSink, RtmpSinkConfig};
use crate::streaming::ffmpeg::fake::FakeEncoder;

const WIDTH: u32 = 2;
const HEIGHT: u32 = 2;
const FRAME_BYTES: usize = (WIDTH * HEIGHT * 4) as usize;
const NANO: Duration = Duration::from_nanos(1);

/// A file that exists, standing in for the video: the fake decoder never
/// reads it, but starting playback checks that the path can be opened.
fn stand_in_video() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")
}

/// The frame period at a rate that divides a second evenly.
fn period(fps: u32) -> Duration {
    Duration::from_nanos(1_000_000_000 / u64::from(fps))
}

type Recorded = Arc<Mutex<Vec<Vec<u8>>>>;

fn recording_target() -> (VideoFrameTarget, Recorded) {
    let recorded: Recorded = Arc::default();
    let sink = recorded.clone();
    let target: VideoFrameTarget = Arc::new(move |frame| {
        sink.lock().unwrap().push(frame.to_vec());
        Ok(())
    });
    (target, recorded)
}

/// The frame number each recorded frame carries, asserting that every frame
/// is whole and filled with a single byte.
fn frame_numbers(recorded: &Recorded) -> Vec<u8> {
    recorded
        .lock()
        .unwrap()
        .iter()
        .map(|frame| {
            assert_eq!(frame.len(), FRAME_BYTES, "frame is a whole RGBA picture");
            assert!(
                frame.iter().all(|byte| *byte == frame[0]),
                "frame is uniformly filled: {frame:?}"
            );
            frame[0]
        })
        .collect()
}

struct VideoRig {
    clock: Arc<SteppedClock>,
    recorded: Recorded,
    handle: VideoPlaybackHandle,
}

impl VideoRig {
    fn start(fps: u32, frames: u8) -> Self {
        let clock = SteppedClock::new();
        let decoders = FakeDecoders::new(&clock, vec![FakeRun::frames(frames, FRAME_BYTES)]);
        let (target, recorded) = recording_target();
        let config = VideoPlaybackConfig {
            ffmpeg_path: "ffmpeg".to_string(),
            path: stand_in_video(),
            width: WIDTH,
            height: HEIGHT,
            fps,
            autoplay: true,
            loop_enabled: false,
        };
        let handle = VideoPlayback::start_with(config, target, clock.runtime(&decoders)).unwrap();
        Self {
            clock,
            recorded,
            handle,
        }
    }
}

impl Drop for VideoRig {
    fn drop(&mut self) {
        self.handle.finish();
    }
}

struct BlackRig {
    clock: Arc<SteppedClock>,
    recorded: Recorded,
    handle: BlackVideoFallbackHandle,
}

impl BlackRig {
    fn start(fps: u32) -> Self {
        let clock = SteppedClock::new();
        let (target, recorded) = recording_target();
        let handle =
            BlackVideoFallback::start_with_clock(WIDTH, HEIGHT, fps, target, clock.clone())
                .unwrap();
        Self {
            clock,
            recorded,
            handle,
        }
    }
}

impl Drop for BlackRig {
    fn drop(&mut self) {
        self.handle.finish();
    }
}

// Background video, worker alone.

fn background_video_paces_one_frame_per_period(fps: u32) {
    let rig = VideoRig::start(fps, 5);
    let period = period(fps);

    rig.clock.settle();
    assert_eq!(frame_numbers(&rig.recorded), [1], "first frame at start");

    rig.clock.advance(period - NANO);
    assert_eq!(
        frame_numbers(&rig.recorded),
        [1],
        "nothing more one nanosecond before the period ends"
    );

    rig.clock.advance(NANO);
    assert_eq!(frame_numbers(&rig.recorded), [1, 2], "second frame");

    rig.clock.advance(period - NANO);
    assert_eq!(frame_numbers(&rig.recorded), [1, 2]);
    rig.clock.advance(NANO);
    assert_eq!(frame_numbers(&rig.recorded), [1, 2, 3], "third frame");

    rig.clock.advance(period - NANO);
    assert_eq!(frame_numbers(&rig.recorded), [1, 2, 3]);
    rig.clock.advance(NANO);
    assert_eq!(frame_numbers(&rig.recorded), [1, 2, 3, 4], "fourth frame");
}

#[test]
fn background_video_delivers_decoded_frames_in_order_one_per_period_at_20_fps() {
    background_video_paces_one_frame_per_period(20);
}

#[test]
fn background_video_delivers_decoded_frames_in_order_one_per_period_at_25_fps() {
    background_video_paces_one_frame_per_period(25);
}

#[test]
fn background_video_does_not_deliver_a_frame_at_a_fraction_of_the_period() {
    let rig = VideoRig::start(20, 3);
    rig.clock.settle();
    let period = period(20);

    // Several small steps that together stay short of a period.
    for _ in 0..4 {
        rig.clock.advance(period / 5);
    }
    assert_eq!(frame_numbers(&rig.recorded), [1]);

    rig.clock.advance(period / 5 - NANO);
    assert_eq!(frame_numbers(&rig.recorded), [1], "one nanosecond short");
    rig.clock.advance(NANO);
    assert_eq!(frame_numbers(&rig.recorded), [1, 2]);
}

#[test]
fn background_video_counts_the_frames_it_delivers() {
    let rig = VideoRig::start(20, 5);
    rig.clock.settle();
    rig.clock.advance(period(20));
    rig.clock.advance(period(20));

    assert_eq!(rig.handle.stats().frames_emitted, 3);
}

// Black fallback, worker alone.

fn black_fallback_paces_one_black_frame_per_period(fps: u32) {
    let rig = BlackRig::start(fps);
    let period = period(fps);

    rig.clock.settle();
    assert_eq!(
        rig.recorded.lock().unwrap().len(),
        1,
        "first frame at start"
    );

    rig.clock.advance(period - NANO);
    assert_eq!(
        rig.recorded.lock().unwrap().len(),
        1,
        "nothing more one nanosecond before the period ends"
    );

    rig.clock.advance(NANO);
    assert_eq!(rig.recorded.lock().unwrap().len(), 2, "second frame");

    rig.clock.advance(period - NANO);
    assert_eq!(rig.recorded.lock().unwrap().len(), 2);
    rig.clock.advance(NANO);
    assert_eq!(rig.recorded.lock().unwrap().len(), 3, "third frame");

    let frames = rig.recorded.lock().unwrap();
    for frame in frames.iter() {
        assert_eq!(frame.len(), FRAME_BYTES, "frame is a whole RGBA picture");
        assert!(frame.iter().all(|byte| *byte == 0), "frame is black");
    }
}

#[test]
fn black_fallback_delivers_black_frames_one_per_period_at_20_fps() {
    black_fallback_paces_one_black_frame_per_period(20);
}

#[test]
fn black_fallback_delivers_black_frames_one_per_period_at_25_fps() {
    black_fallback_paces_one_black_frame_per_period(25);
}

#[test]
fn black_fallback_does_not_deliver_a_frame_at_a_fraction_of_the_period() {
    let rig = BlackRig::start(20);
    rig.clock.settle();
    let period = period(20);

    for _ in 0..4 {
        rig.clock.advance(period / 5);
    }
    assert_eq!(rig.recorded.lock().unwrap().len(), 1);

    rig.clock.advance(period / 5 - NANO);
    assert_eq!(
        rig.recorded.lock().unwrap().len(),
        1,
        "one nanosecond short"
    );
    rig.clock.advance(NANO);
    assert_eq!(rig.recorded.lock().unwrap().len(), 2);
}

// Through the RTMP sink. The stream's encoder worker runs when the sink
// finishes, so each case runs a fresh sink through its clock steps, finishes
// it, and reads the frames its encoder session received.

fn sink_config(fps: u32, background: bool) -> RtmpSinkConfig {
    let mut config = json!({
        "url": "rtmp://example.test/live/fugue",
        "width": WIDTH,
        "height": HEIGHT,
        "fps": fps,
        "video_queue_frames": 32
    });
    if background {
        config["background_video"] = json!(stand_in_video().to_str().unwrap());
    }
    RtmpSinkConfig::from_json(&config, 48_000).unwrap()
}

/// The video frames the stream's encoder session received after the clock
/// is settled at start and then advanced by each of `steps` in turn.
fn frames_at_the_stream(fps: u32, background: bool, steps: &[Duration]) -> Vec<Vec<u8>> {
    let clock = SteppedClock::new();
    let decoders = FakeDecoders::new(&clock, vec![FakeRun::frames(8, FRAME_BYTES)]);
    let fake = FakeEncoder::default();
    let (_sink, handle, _stream) = RtmpSink::with_fakes(
        sink_config(fps, background),
        &fake,
        !background,
        clock.runtime(&decoders),
    )
    .unwrap();

    clock.settle();
    for step in steps {
        clock.advance(*step);
    }
    handle.finish();

    fake.sessions()
        .into_iter()
        .flat_map(|session| session.video)
        .collect()
}

fn numbers(frames: &[Vec<u8>]) -> Vec<u8> {
    frames
        .iter()
        .map(|frame| {
            assert_eq!(frame.len(), FRAME_BYTES, "frame is a whole RGBA picture");
            assert!(frame.iter().all(|byte| *byte == frame[0]), "uniform frame");
            frame[0]
        })
        .collect()
}

#[test]
fn sink_streams_background_video_frames_in_order_one_per_period() {
    let fps = 20;
    let period = period(fps);

    let at_start = frames_at_the_stream(fps, true, &[]);
    assert_eq!(numbers(&at_start), [1], "first frame at start");

    let just_short = frames_at_the_stream(fps, true, &[period - NANO]);
    assert_eq!(
        numbers(&just_short),
        [1],
        "nothing more before a full period"
    );

    let one_period = frames_at_the_stream(fps, true, &[period - NANO, NANO]);
    assert_eq!(numbers(&one_period), [1, 2]);

    let second_short = frames_at_the_stream(fps, true, &[period - NANO, NANO, period - NANO]);
    assert_eq!(numbers(&second_short), [1, 2]);

    let two_periods = frames_at_the_stream(fps, true, &[period - NANO, NANO, period]);
    assert_eq!(numbers(&two_periods), [1, 2, 3]);

    let four_periods = frames_at_the_stream(fps, true, &[period, period, period]);
    assert_eq!(numbers(&four_periods), [1, 2, 3, 4]);
}

#[test]
fn sink_streams_black_frames_one_per_period_without_background_video() {
    let fps = 20;
    let period = period(fps);

    let at_start = frames_at_the_stream(fps, false, &[]);
    assert_eq!(at_start.len(), 1, "first frame at start");

    let just_short = frames_at_the_stream(fps, false, &[period - NANO]);
    assert_eq!(just_short.len(), 1, "nothing more before a full period");

    let one_period = frames_at_the_stream(fps, false, &[period - NANO, NANO]);
    assert_eq!(one_period.len(), 2);

    let second_short = frames_at_the_stream(fps, false, &[period - NANO, NANO, period - NANO]);
    assert_eq!(second_short.len(), 2);

    let four_periods = frames_at_the_stream(fps, false, &[period, period, period]);
    assert_eq!(four_periods.len(), 4);

    for frame in four_periods {
        assert_eq!(frame.len(), FRAME_BYTES, "frame is a whole RGBA picture");
        assert!(frame.iter().all(|byte| *byte == 0), "frame is black");
    }
}

#[test]
fn sink_follows_the_configured_fps_for_background_video() {
    let period = period(25);
    assert_eq!(
        numbers(&frames_at_the_stream(25, true, &[period - NANO])),
        [1]
    );
    assert_eq!(
        numbers(&frames_at_the_stream(25, true, &[period - NANO, NANO])),
        [1, 2]
    );
}

#[test]
fn sink_follows_the_configured_fps_for_black_frames() {
    let period = period(25);
    assert_eq!(frames_at_the_stream(25, false, &[period - NANO]).len(), 1);
    assert_eq!(
        frames_at_the_stream(25, false, &[period - NANO, NANO]).len(),
        2
    );
}
