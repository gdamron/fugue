//! Acceptance tests for the RTMP sink's stream feed, run over a recording
//! fake encoder: no ffmpeg, no subprocess, no sleeping.

use std::sync::Arc;

use serde_json::json;

use super::{RtmpSink, RtmpSinkConfig, RtmpSinkHandle};
use crate::modules::dac::soft_clip;
use crate::streaming::ffmpeg::fake::{stop_when_idle, FakeEncoder, FakeSession};
use crate::streaming::ffmpeg::{
    FfmpegStreamBackend, FfmpegStreamConfig, FfmpegStreamHandle, Pause,
    DEFAULT_AUDIO_BUFFER_FRAMES, DEFAULT_VIDEO_QUEUE_FRAMES,
};
use crate::{Module, SinkModule};

const LEFT_INPUT: usize = 1;
const RIGHT_INPUT: usize = 2;

fn sink_config() -> RtmpSinkConfig {
    RtmpSinkConfig::from_json(
        &json!({
            "url": "rtmp://example.test/live/fugue",
            "width": 640,
            "height": 360
        }),
        48_000,
    )
    .unwrap()
}

/// A monitored sink over `fake`, with soft clip set as given.
fn sink_over(
    fake: &FakeEncoder,
    soft_clip: bool,
) -> (RtmpSink, RtmpSinkHandle, FfmpegStreamHandle) {
    let (sink, handle, stream) = RtmpSink::with_fake_stream(sink_config(), fake).unwrap();
    (
        sink.with_soft_clip(soft_clip).with_monitor(true),
        handle,
        stream,
    )
}

/// Writes `left` and `right` to the sink's inputs and processes one block.
fn process_block(sink: &mut RtmpSink, left: &[f32], right: &[f32]) {
    assert_eq!(left.len(), right.len());
    sink.input_block_mut(LEFT_INPUT)[..left.len()].copy_from_slice(left);
    sink.input_block_mut(RIGHT_INPUT)[..right.len()].copy_from_slice(right);
    sink.process(left.len());
}

fn zip_frames(left: &[f32], right: &[f32]) -> Vec<(f32, f32)> {
    left.iter().copied().zip(right.iter().copied()).collect()
}

fn assert_bit_identical(actual: &[(f32, f32)], expected: &[(f32, f32)]) {
    assert_eq!(actual.len(), expected.len(), "frame count");
    for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(a.0.to_bits(), e.0.to_bits(), "left at frame {i}");
        assert_eq!(a.1.to_bits(), e.1.to_bits(), "right at frame {i}");
    }
}

// Stream backend with a small video frame, for the worker-level tests.
const VIDEO_WIDTH: u32 = 6;
const VIDEO_HEIGHT: u32 = 5;
const VIDEO_FRAME_BYTES: usize = (VIDEO_WIDTH * VIDEO_HEIGHT * 4) as usize;

fn backend_config() -> FfmpegStreamConfig {
    FfmpegStreamConfig {
        ffmpeg_path: "ffmpeg".to_string(),
        url: "rtmp://example.test/live/fugue".to_string(),
        tee_to_disk: None,
        width: VIDEO_WIDTH,
        height: VIDEO_HEIGHT,
        fps: 30,
        sample_rate: 48_000,
        video_encoder: "libx264".to_string(),
        audio_encoder: "aac".to_string(),
        video_bitrate: "1000k".to_string(),
        audio_bitrate: "128k".to_string(),
        gop_duration: 2,
        constant_video_bitrate: false,
        audio_buffer_frames: DEFAULT_AUDIO_BUFFER_FRAMES,
        video_queue_frames: DEFAULT_VIDEO_QUEUE_FRAMES,
    }
}

fn backend_over(
    fake: &FakeEncoder,
    pause: impl FnMut(&FfmpegStreamBackend, Pause) + Send + 'static,
) -> FfmpegStreamHandle {
    FfmpegStreamBackend::start_on_finish(backend_config(), Arc::new(fake.clone()), pause)
}

#[test]
fn non_finite_samples_reach_the_stream_as_silence_and_are_counted() {
    let fake = FakeEncoder::default();
    let (mut sink, handle, _stream) = sink_over(&fake, false);
    let mut left: Vec<f32> = (0..16).map(|i| 0.1 + i as f32 * 0.01).collect();
    let mut right: Vec<f32> = (0..16).map(|i| -0.1 - i as f32 * 0.01).collect();
    left[3] = f32::NAN;
    right[7] = f32::INFINITY;
    left[11] = f32::NEG_INFINITY;
    let before = sink.non_finite_samples();

    process_block(&mut sink, &left, &right);
    let after = sink.non_finite_samples();
    handle.finish();

    assert_eq!(after - before, 3);
    let frames = fake.audio_frames();
    assert_eq!(frames.len(), 16);
    assert_eq!(frames[3].0, 0.0);
    assert_eq!(frames[7].1, 0.0);
    assert_eq!(frames[11].0, 0.0);
    for (i, frame) in frames.iter().enumerate() {
        assert!(frame.0.is_finite() && frame.1.is_finite(), "frame {i}");
        if ![3, 11].contains(&i) {
            assert_eq!(frame.0, left[i], "left at frame {i}");
        }
        if i != 7 {
            assert_eq!(frame.1, right[i], "right at frame {i}");
        }
    }
}

#[test]
fn finite_samples_outside_unit_range_reach_the_stream_unchanged_without_soft_clip() {
    let fake = FakeEncoder::default();
    let (mut sink, handle, _stream) = sink_over(&fake, false);
    let left = [2.0, -3.5, 0.25, 1.0000001, 0.0];
    let right = [-2.0, 3.5, -0.25, -1.0000001, 100.0];

    process_block(&mut sink, &left, &right);
    handle.finish();

    assert_bit_identical(&fake.audio_frames(), &zip_frames(&left, &right));
}

#[test]
fn soft_clip_shapes_the_stream_while_the_monitor_keeps_the_unclipped_signal() {
    let fake = FakeEncoder::default();
    let (mut sink, handle, _stream) = sink_over(&fake, true);
    let left = [2.0, -3.5, 0.25, 0.9, -1.2];
    let right = [-2.0, 3.5, -0.25, 1.5, 0.0];

    process_block(&mut sink, &left, &right);
    let (monitor_left, monitor_right) = sink.sink_block();
    assert_bit_identical(
        &zip_frames(&monitor_left[..5], &monitor_right[..5]),
        &zip_frames(&left, &right),
    );
    handle.finish();

    let expected: Vec<(f32, f32)> = left
        .iter()
        .zip(&right)
        .map(|(&l, &r)| (soft_clip(l), soft_clip(r)))
        .collect();
    assert_ne!(expected[0].0, left[0], "soft clip must change 2.0");
    assert_bit_identical(&fake.audio_frames(), &expected);
}

#[test]
fn left_and_right_reach_the_stream_in_input_order() {
    let fake = FakeEncoder::default();
    let (mut sink, handle, _stream) = sink_over(&fake, false);
    let left: Vec<f32> = (0..256).map(|i| (i + 1) as f32 * 0.001).collect();
    let right: Vec<f32> = (0..256).map(|i| -((i + 1) as f32) * 0.002).collect();

    process_block(&mut sink, &left, &right);
    handle.finish();

    assert_bit_identical(&fake.audio_frames(), &zip_frames(&left, &right));
}

#[test]
fn a_stopping_sink_adds_no_frames_to_the_stream() {
    let fake = FakeEncoder::default();
    let (mut sink, handle, stream) = sink_over(&fake, false);
    let block = [0.5f32; 64];
    process_block(&mut sink, &block, &block);
    stream.stop();
    assert!(stream.is_stopping());

    process_block(&mut sink, &block, &block);
    process_block(&mut sink, &block, &block);
    handle.finish();

    assert_eq!(fake.audio_frames().len(), 64, "only the pre-stop block");
    assert_eq!(handle.stats().audio_frames_sent, 64);
}

#[test]
fn a_sink_processed_after_finish_leaves_the_stream_empty() {
    let fake = FakeEncoder::default();
    let (mut sink, handle, stream) = sink_over(&fake, false);
    handle.finish();
    let block = [0.5f32; 64];

    process_block(&mut sink, &block, &block);

    assert!(stream.audio_ring_is_empty());
    assert!(fake.audio_frames().is_empty());
    assert_eq!(handle.stats().audio_frames_sent, 0);
}

#[test]
fn every_frame_pushed_before_finish_is_delivered_before_the_stream_closes() {
    let fake = FakeEncoder::default();
    let (mut sink, handle, _stream) = sink_over(&fake, false);
    let mut pushed = 0usize;
    let mut expected = Vec::new();
    for block in 0..10 {
        let left: Vec<f32> = (0..200)
            .map(|i| (block * 200 + i + 1) as f32 * 1e-4)
            .collect();
        let right: Vec<f32> = left.iter().map(|v| -v).collect();
        process_block(&mut sink, &left, &right);
        expected.extend(zip_frames(&left, &right));
        pushed += left.len();
    }

    let stats = handle.finish();

    assert_eq!(stats.audio_frames_sent, pushed);
    assert_eq!(stats.audio_frames_dropped, 0);
    assert_bit_identical(&fake.audio_frames(), &expected);
    let sessions = fake.sessions();
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].closed, "session closed after the last frame");
}

#[test]
fn frames_pushed_straight_to_the_backend_are_all_delivered_on_finish() {
    let fake = FakeEncoder::default();
    let stream = backend_over(&fake, stop_when_idle);
    for i in 0..500 {
        stream.push_audio(i as f32, -(i as f32));
    }

    stream.finish();

    let stats = stream.stats();
    assert_eq!(stats.audio_frames_sent, 500);
    assert_eq!(stats.audio_frames_dropped, 0);
    assert_eq!(fake.audio_frames().len(), 500);
}

#[test]
fn an_unexpected_encoder_exit_starts_a_second_session_that_receives_later_frames() {
    let fake = FakeEncoder::scripted([
        FakeSession::ExitOnFirstCheck("encoder exited".to_string()),
        FakeSession::Run,
    ]);
    let mut pushed_after_restart = false;
    let stream = backend_over(&fake, move |backend, pause| match pause {
        Pause::AfterExit if !pushed_after_restart => {
            pushed_after_restart = true;
            for i in 0..50 {
                backend.push_audio(i as f32 + 1.0, -(i as f32) - 1.0);
            }
        }
        _ => stop_when_idle(backend, pause),
    });

    stream.run_worker();

    let stats = stream.stats();
    assert_eq!(stats.restarts, 1);
    assert!(stats.last_error.is_some(), "last_error is set");
    assert_eq!(fake.starts(), 2);
    let sessions = fake.sessions();
    assert_eq!(sessions.len(), 2);
    assert!(sessions[0].audio.is_empty(), "nothing reached session one");
    let expected: Vec<(f32, f32)> = (0..50)
        .map(|i| (i as f32 + 1.0, -(i as f32) - 1.0))
        .collect();
    assert_eq!(sessions[1].audio_frames(), expected);
}

#[test]
fn an_encoder_that_fails_to_start_counts_a_restart_and_names_the_failure() {
    let fake = FakeEncoder::scripted([FakeSession::FailToStart("encoder not found".to_string())]);
    let stream = backend_over(&fake, |backend, pause| {
        if pause == Pause::AfterStartFailure {
            backend.stop();
        }
    });

    stream.run_worker();

    let stats = stream.stats();
    assert!(stats.restarts >= 1, "restarts: {}", stats.restarts);
    let error = stats.last_error.expect("last_error is set");
    assert!(error.contains("encoder not found"), "last_error: {error}");
    assert!(fake.starts() >= 1);
}

#[test]
fn a_video_frame_of_the_wrong_size_is_rejected_naming_the_expected_size() {
    let fake = FakeEncoder::default();
    let stream = backend_over(&fake, stop_when_idle);

    let error = stream
        .push_video_rgba(&[0u8; VIDEO_FRAME_BYTES - 4])
        .expect_err("a short frame must be rejected");

    assert!(
        error.contains(&VIDEO_FRAME_BYTES.to_string()) || error.contains("6x5"),
        "error does not name the expected size: {error}"
    );
    let ok = stream.push_video_rgba(&[0u8; VIDEO_FRAME_BYTES]);
    assert!(ok.is_ok(), "a correctly sized frame is accepted: {ok:?}");
}

#[test]
fn a_video_frame_of_the_wrong_size_is_rejected_through_the_sink_handle() {
    let fake = FakeEncoder::default();
    let (_sink, handle, _stream) = sink_over(&fake, false);
    let expected_bytes = 640 * 360 * 4;

    let error = handle
        .push_video_rgba(&vec![0u8; expected_bytes + 1])
        .expect_err("an oversized frame must be rejected");

    assert!(
        error.contains(&expected_bytes.to_string()) || error.contains("640x360"),
        "error does not name the expected size: {error}"
    );
}
