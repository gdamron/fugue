//! What the sink sends to its stream, checked against the recording fake
//! encoder and fake video: no process, socket or sleep.

use serde_json::json;

use super::*;
use crate::streaming::ffmpeg::fake::FakeEncoder;
use crate::streaming::ffmpeg::FfmpegStreamHandle;
use crate::streaming::video::fake::{FakeDecoders, FakeRun, SteppedClock};

const LEFT: usize = 1;
const RIGHT: usize = 2;

fn sink(soft_clip: bool) -> (RtmpSink, RtmpSinkHandle, FfmpegStreamHandle, FakeEncoder) {
    let config = RtmpSinkConfig::from_json(
        &json!({
            "url": "rtmp://example.test/live/fugue",
            "width": 2,
            "height": 2,
            "buffer_frames": 4_096,
            "video_queue_frames": 2,
            "monitor": true,
            "soft_clip": soft_clip
        }),
        48_000,
    )
    .unwrap();
    let fake = FakeEncoder::default();
    let (sink, handle, stream) = RtmpSink::with_fake_stream(config, &fake).unwrap();
    (sink, handle, stream, fake)
}

fn process_block(sink: &mut RtmpSink, left: &[f32], right: &[f32]) {
    sink.input_block_mut(LEFT)[..left.len()].copy_from_slice(left);
    sink.input_block_mut(RIGHT)[..right.len()].copy_from_slice(right);
    sink.process(left.len());
}

#[test]
fn non_finite_samples_reach_the_stream_as_silence() {
    let (mut sink, handle, _, fake) = sink(false);
    let left = [0.5, f32::NAN, 0.25, f32::NEG_INFINITY];
    let right = [f32::INFINITY, -0.5, 0.75, 0.125];
    process_block(&mut sink, &left, &right);
    handle.finish();

    // Bit patterns, so a negative zero would fail.
    let bits: Vec<_> = fake
        .audio_frames()
        .iter()
        .map(|(l, r)| (l.to_bits(), r.to_bits()))
        .collect();
    let expected: Vec<_> = [(0.5f32, 0.0f32), (0.0, -0.5), (0.25, 0.75), (0.0, 0.125)]
        .iter()
        .map(|(l, r)| (l.to_bits(), r.to_bits()))
        .collect();
    assert_eq!(bits, expected);
    assert_eq!(sink.non_finite_samples(), 3);
}

#[test]
fn finite_samples_reach_the_stream_bit_identical_without_soft_clip() {
    let (mut sink, handle, _, fake) = sink(false);
    let left = [2.0, -3.5, 0.1, f32::MIN_POSITIVE];
    let right = [-2.0, 3.5, -0.3, 1.0e-30];
    process_block(&mut sink, &left, &right);
    handle.finish();

    let sent = fake.audio_frames();
    let bits = |frames: &[(f32, f32)]| -> Vec<(u32, u32)> {
        frames
            .iter()
            .map(|(left, right)| (left.to_bits(), right.to_bits()))
            .collect()
    };
    let expected: Vec<_> = left.into_iter().zip(right).collect();
    assert_eq!(bits(&sent), bits(&expected));
}

#[test]
fn soft_clip_shapes_the_stream_but_not_the_monitor() {
    let (mut sink, handle, _, fake) = sink(true);
    let left = [2.0, 0.5, -4.0];
    let right = [-2.0, -0.25, 1.5];
    process_block(&mut sink, &left, &right);
    handle.finish();

    let expected: Vec<_> = left
        .iter()
        .zip(&right)
        .map(|(&left, &right)| {
            (
                crate::modules::dac::soft_clip(left),
                crate::modules::dac::soft_clip(right),
            )
        })
        .collect();
    assert_eq!(fake.audio_frames(), expected);
    assert_ne!(expected[0].0, 2.0);
    let (monitor_left, monitor_right) = sink.sink_block();
    assert_eq!(monitor_left[..3], left);
    assert_eq!(monitor_right[..3], right);
    assert_eq!(sink.get_output("audio_left").unwrap(), 2.0);
}

#[test]
fn channels_reach_the_stream_in_order_and_never_swapped() {
    let (mut sink, handle, _, fake) = sink(false);
    let left: Vec<f32> = (0..256).map(|i| i as f32).collect();
    let right: Vec<f32> = (0..256).map(|i| 1_000.0 + i as f32).collect();
    process_block(&mut sink, &left, &right);
    handle.finish();

    let expected: Vec<_> = left.into_iter().zip(right).collect();
    assert_eq!(fake.audio_frames(), expected);
}

#[test]
fn stopping_sink_adds_no_frames_to_the_stream() {
    let (mut sink, handle, stream, fake) = sink(false);
    process_block(&mut sink, &[0.5; 4], &[-0.5; 4]);
    let stats = handle.finish();
    assert_eq!(stats.audio_frames_sent, 4);

    process_block(&mut sink, &[0.25; 8], &[0.25; 8]);
    let (monitor_left, _) = sink.sink_block();
    assert_eq!(monitor_left[..8], [0.25; 8]);
    assert!(stream.audio_ring_is_empty());
    assert_eq!(fake.audio_frames().len(), 4);
    assert_eq!(handle.stats().audio_frames_dropped, 0);
}

#[test]
fn a_stream_told_to_stop_takes_no_more_frames_from_the_sink() {
    let (mut sink, handle, stream, fake) = sink(false);
    process_block(&mut sink, &[0.5; 64], &[0.5; 64]);
    stream.stop();
    assert!(stream.is_stopping());

    process_block(&mut sink, &[0.5; 64], &[0.5; 64]);
    process_block(&mut sink, &[0.5; 64], &[0.5; 64]);
    handle.finish();

    assert_eq!(fake.audio_frames().len(), 64, "only the block before stop");
    assert_eq!(handle.stats().audio_frames_sent, 64);
}

#[test]
fn finish_delivers_every_frame_pushed_before_it() {
    let (mut sink, handle, _, fake) = sink(false);
    let mut pushed = 0;
    for block in 0..6 {
        let left = [block as f32; 512];
        process_block(&mut sink, &left, &left);
        pushed += left.len();
    }
    let stats = handle.finish();

    let sessions = fake.sessions();
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].closed);
    let expected: Vec<_> = (0..6)
        .flat_map(|block| [(block as f32, block as f32); 512])
        .collect();
    assert_eq!(
        sessions[0].audio_frames(),
        expected,
        "every block, in order"
    );
    assert_eq!(expected.len(), pushed);
    assert_eq!(stats.audio_frames_sent, pushed, "{stats:?}");
    assert_eq!(stats.audio_frames_dropped, 0, "{stats:?}");
}

#[test]
fn wrong_size_video_frame_names_the_expected_size() {
    let (_sink, handle, _, fake) = sink(false);
    for wrong in [&[0; 15][..], &[0; 17][..]] {
        let err = handle.push_video_rgba(wrong).unwrap_err();
        assert!(err.contains("RGBA frame with 16 bytes"), "{err}");
    }

    handle.push_video_rgba(&[7; 16]).unwrap();
    let stats = handle.finish();
    assert_eq!(fake.sessions()[0].video, vec![vec![7; 16]]);
    assert_eq!(stats.video_frames_sent, 1);
}

#[test]
fn configured_background_video_feeds_stream_backend() {
    let config = RtmpSinkConfig::from_json(
        &json!({
            "url": "rtmp://example.test/live/fugue",
            "width": 2,
            "height": 2,
            "fps": 20,
            "buffer_frames": 64,
            "video_queue_frames": 2,
            // Any existing file: the fake decoder never opens it.
            "background_video": concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")
        }),
        48_000,
    )
    .unwrap();
    let fake = FakeEncoder::default();
    let clock = SteppedClock::new();
    let decoders = FakeDecoders::new(&clock, vec![FakeRun::frames(3, 16)]);
    let (_sink, handle, _) =
        RtmpSink::with_fakes(config, &fake, false, clock.runtime(&decoders)).unwrap();
    assert!(handle.has_background_video());

    // 20 fps: frame 2 exactly one period in, frame 3 not before the next.
    let period = std::time::Duration::from_millis(50);
    let nano = std::time::Duration::from_nanos(1);
    clock.settle();
    clock.advance(period - nano);
    clock.advance(nano);
    clock.advance(period - nano);
    let stats = handle.finish();
    assert_eq!(fake.sessions()[0].video, vec![vec![1; 16], vec![2; 16]]);
    assert_eq!(stats.video_frames_sent, 2, "{stats:?}");
    assert_eq!(stats.video_frames_dropped, 0, "{stats:?}");
}
