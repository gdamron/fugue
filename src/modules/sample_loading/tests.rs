use std::sync::Arc;

use super::elastic::ElasticReader;
use super::{elastic_mode_from_config, SampleData};

const RATE: u32 = 44_100;

/// Mono test signal: a sine with a slow amplitude ramp, so no two windows
/// are proportional and the alignment search has a unique best offset.
fn ramped_sine(len: usize, period: f32) -> Arc<SampleData> {
    let samples: Vec<f32> = (0..len)
        .map(|i| {
            let amp = 0.2 + 0.8 * (i as f32 / len as f32);
            amp * (std::f32::consts::TAU * i as f32 / period).sin()
        })
        .collect();
    Arc::new(SampleData::from_interleaved(1, RATE, RATE, samples))
}

/// Pulls up to `max` frames at fixed ratios, stopping at region end.
fn render(
    reader: &mut ElasticReader,
    sample: &Arc<SampleData>,
    region_end: f64,
    time_ratio: f32,
    pitch_ratio: f32,
    max: usize,
) -> Vec<(f32, f32)> {
    let analysis = sample.elastic_analysis();
    let mut frames = Vec::new();
    for _ in 0..max {
        match reader.next(sample, &analysis, 0.0, region_end, time_ratio, pitch_ratio) {
            Some(frame) => frames.push(frame),
            None => break,
        }
    }
    frames
}

#[test]
fn elastic_analysis_is_computed_once_per_asset() {
    let sample = ramped_sine(4096, 64.0);
    assert!(sample.cached_elastic_analysis().is_none());

    let first = sample.elastic_analysis();
    let second = sample.elastic_analysis();
    assert!(Arc::ptr_eq(&first, &second));
    assert!(sample.cached_elastic_analysis().is_some());
}

#[test]
fn unity_ratios_reconstruct_the_source() {
    let len = 16_384;
    let sample = ramped_sine(len, 64.0);
    let mut reader = ElasticReader::new(RATE);
    reader.reset(0.0);

    let out = render(&mut reader, &sample, len as f64, 1.0, 1.0, 8192);
    assert_eq!(out.len(), 8192);

    // A fresh reader starts at full amplitude and every later window aligns
    // to its natural continuation, so unity playback matches the source.
    let mut max_err = 0.0f32;
    for (i, (l, _)) in out.iter().enumerate() {
        max_err = max_err.max((l - sample.left[i]).abs());
    }
    assert!(max_err < 1e-3, "unity reconstruction error {}", max_err);
}

#[test]
fn time_ratio_scales_output_duration_exactly() {
    let len = 6000;
    let sample = ramped_sine(len, 64.0);

    let mut reader = ElasticReader::new(RATE);
    reader.reset(0.0);
    let fast = render(&mut reader, &sample, len as f64, 2.0, 1.0, 20_000);
    assert_eq!(fast.len(), 3000, "2x speed halves the output length");

    reader.reset(0.0);
    let slow = render(&mut reader, &sample, len as f64, 0.5, 1.0, 20_000);
    assert_eq!(slow.len(), 12_000, "0.5x speed doubles the output length");
}

#[test]
fn pitch_ratio_shifts_pitch_without_changing_duration() {
    let len = 16_384;
    let period = 64.0;
    let sample = ramped_sine(len, period);

    let mut reader = ElasticReader::new(RATE);
    reader.reset(0.0);
    let out = render(&mut reader, &sample, len as f64, 1.0, 2.0, len + 10);
    // Duration is governed by time_ratio alone.
    assert_eq!(out.len(), len);

    // Count zero crossings over a window where all reads stay in the
    // source (pitch 2.0 reads roughly twice as far as it emits).
    let count_crossings = |signal: &[f32]| {
        signal
            .windows(2)
            .filter(|pair| (pair[0] >= 0.0) != (pair[1] >= 0.0))
            .count() as f32
    };
    let out_left: Vec<f32> = out[256..4096].iter().map(|frame| frame.0).collect();
    let source_crossings = count_crossings(&sample.left[256..4096]);
    let out_crossings = count_crossings(&out_left);
    let ratio = out_crossings / source_crossings;
    assert!(
        (ratio - 2.0).abs() < 0.3,
        "expected ~2x zero-crossing rate, got {}x",
        ratio
    );
}

/// Regression test for continuation-chasing: off unity, the alignment
/// search must not ride the previous window's exact continuation (which
/// plays at native rate and then snaps back — an audible rate sawtooth with
/// a pop at every resync). Windows must instead advance near the nominal
/// stretched rate every single cycle.
#[test]
fn slowdown_advances_windows_at_a_steady_stretched_rate() {
    let len = 32_768;
    let sample = ramped_sine(len, 64.0);
    let analysis = sample.elastic_analysis();
    let mut reader = ElasticReader::new(RATE);
    reader.reset(0.0);

    let time_ratio = 0.7f32;
    let mut bases: Vec<f64> = Vec::new();
    let mut frames_between: Vec<usize> = Vec::new();
    let mut since_last = 0usize;
    for _ in 0..14_000 {
        reader
            .next(&sample, &analysis, 0.0, len as f64, time_ratio, 1.0)
            .unwrap();
        since_last += 1;
        if bases.last() != Some(&reader.last_base()) {
            bases.push(reader.last_base());
            frames_between.push(since_last);
            since_last = 0;
        }
    }
    assert!(bases.len() >= 5, "expected several windows, got {bases:?}");

    // Every window advance must sit near cycle * time_ratio; the chase bug
    // produced advances alternating between ~cycle (native rate) and a
    // compensating snap-back.
    let cycle = frames_between[2] as f64;
    let nominal = cycle * f64::from(time_ratio);
    for pair in bases.windows(2).skip(1) {
        let advance = pair[1] - pair[0];
        assert!(
            (advance - nominal).abs() < 200.0,
            "window advance {} strayed from nominal {} (bases {:?})",
            advance,
            nominal,
            bases
        );
    }
}

#[test]
fn region_end_stops_the_reader_and_reset_restarts_it() {
    let sample = ramped_sine(4096, 64.0);
    let mut reader = ElasticReader::new(RATE);
    reader.reset(1000.0);

    let out = render(&mut reader, &sample, 1500.0, 1.0, 1.0, 10_000);
    assert_eq!(
        out.len(),
        500,
        "region of 500 frames at 1x emits 500 frames"
    );
    assert!(reader.source_position() >= 1500.0);

    reader.reset_crossfade(1000.0);
    assert_eq!(reader.source_position(), 1000.0);
    let again = render(&mut reader, &sample, 1500.0, 1.0, 1.0, 10_000);
    assert_eq!(again.len(), 500);
}

#[test]
fn elastic_mode_config_parses_and_rejects_unknowns() {
    let none = serde_json::json!({});
    assert_eq!(elastic_mode_from_config("m", &none), Ok(false));
    let classic = serde_json::json!({ "mode": "classic" });
    assert_eq!(elastic_mode_from_config("m", &classic), Ok(false));
    let elastic = serde_json::json!({ "mode": "elastic" });
    assert_eq!(elastic_mode_from_config("m", &elastic), Ok(true));
    let bad = serde_json::json!({ "mode": "wsola" });
    let err = elastic_mode_from_config("m", &bad).unwrap_err();
    assert!(err.contains("'mode'"), "{err}");
}

#[test]
fn a_sample_decoded_by_a_losing_racer_is_not_the_one_returned() {
    // Two threads missed the same key, and both decoded it: the second to
    // cache gets the first's buffer, so the cache owns whatever either
    // returns, and a voice dropping it on the audio thread frees nothing.
    let key = ("fugue-test://a-raced-sample".to_string(), RATE, None);
    let first = Arc::new(SampleData::from_interleaved(1, RATE, RATE, vec![0.1; 4]));
    let second = Arc::new(SampleData::from_interleaved(1, RATE, RATE, vec![0.2; 4]));
    let lost = Arc::downgrade(&second);
    assert!(Arc::ptr_eq(
        &super::cache_loaded(key.clone(), first.clone()),
        &first
    ));
    assert!(Arc::ptr_eq(&super::cache_loaded(key, second), &first));
    assert!(lost.upgrade().is_none(), "the losing decode is dropped");
}

#[test]
fn a_changed_file_at_the_same_path_decodes_afresh() {
    // A source-checkout package (or an edited local sample) keeps its path
    // when its bytes change; the cache must not replay the first decode.
    fn write(path: &std::path::Path, value: f32) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for _ in 0..8 {
            writer.write_sample(value).unwrap();
        }
        writer.finalize().unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hit.wav");
    let source = path.to_str().unwrap();
    write(&path, 0.25);
    let first = super::load_cached_sample(source, RATE).unwrap();
    assert!(Arc::ptr_eq(
        &first,
        &super::load_cached_sample(source, RATE).unwrap()
    ));
    write(&path, 0.5);
    let second = super::load_cached_sample(source, RATE).unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(second.frame_at(0).0, 0.5);

    // A revision still held elsewhere stays cached (its last drop must not
    // free it, as that drop may run on the audio thread); one nothing else
    // holds is reclaimed when a newer revision is cached.
    assert_eq!(Arc::strong_count(&first), 2);
    let first_gone = Arc::downgrade(&first);
    drop(first);
    write(&path, 0.75);
    let third = super::load_cached_sample(source, RATE).unwrap();
    assert_eq!(third.frame_at(0).0, 0.75);
    assert!(first_gone.upgrade().is_none());
    assert_eq!(Arc::strong_count(&second), 2);
}

#[test]
fn oversized_and_non_regular_local_samples_are_refused_before_reading() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.wav");
    std::fs::write(&path, vec![0u8; 64]).unwrap();
    let source = path.to_str().unwrap();
    let refused = |source: &str| match super::load_cached_sample(source, RATE) {
        Ok(_) => panic!("{source} loaded"),
        Err(error) => error,
    };
    let error = crate::pkg::read_limit::test_override::with_hashed_limit(16, || refused(source));
    assert!(error.contains("Failed to open sample"), "{error}");

    let error = refused(dir.path().to_str().unwrap());
    assert!(error.contains("not a regular file"), "{error}");
    #[cfg(unix)]
    {
        let error = refused("/dev/zero");
        assert!(error.contains("not a regular file"), "{error}");
    }
}
