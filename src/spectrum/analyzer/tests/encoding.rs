//! How tiles carry their levels on the wire.

use super::*;

/// Decodes a byte-encoded tile back to decibels, as a viewer would.
fn decode(tile: &SpectrogramTile, meta: &SpectrogramStreamMeta) -> Vec<f32> {
    let SpectrogramMagnitudes::Text(text) = &tile.magnitudes else {
        panic!("expected base64 levels");
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text)
        .expect("valid base64");
    let (floor, ceiling) = (meta.db.floor_db, meta.db.ceiling_db);
    bytes
        .iter()
        .map(|q| floor + *q as f32 / 255.0 * (ceiling - floor))
        .collect()
}

/// The same audio analysed under both encodings.
fn both_encodings(feed: impl Fn(&SpectrumTap)) -> (Vec<f32>, SpectrogramTile, SpectrumAnalyzer) {
    let (json_tap, mut json) = analyzer(config());
    let (byte_tap, mut bytes) = analyzer(SpectrumConfig {
        encoding: SpectrogramEncoding::U8Base64,
        ..config()
    });
    feed(&json_tap);
    feed(&byte_tap);
    let reference = levels(&json.poll().unwrap()).to_vec();
    let tile = bytes.poll().unwrap();
    (reference, tile, bytes)
}

#[test]
fn byte_levels_decode_to_within_half_a_step() {
    let (reference, tile, analyzer) = both_encodings(|tap| {
        let mut phase = 0.0;
        feed_tone(tap, 3_000.0, 0.3, 1_024, &mut phase);
    });
    assert!(tile.matches(analyzer.meta()));
    let decoded = decode(&tile, analyzer.meta());
    assert_eq!(decoded.len(), reference.len());
    // A step is 100 dB / 255; rounding to it, plus the reference's own 0.1 dB
    // rounding, keeps every level within about a quarter of a decibel.
    for (bin, (got, want)) in decoded.iter().zip(&reference).enumerate() {
        assert!((got - want).abs() < 0.25, "bin {bin}: {got} vs {want}");
    }
}

#[test]
fn byte_levels_clamp_to_the_declared_range() {
    // Well over full scale, at DC.
    let (_, tile, analyzer) = both_encodings(|tap| feed_constant(tap, 8.0, 1_024));
    let decoded = decode(&tile, analyzer.meta());
    assert_eq!(decoded[0], 0.0, "louder than the ceiling reads the ceiling");

    let (_, tile, analyzer) = both_encodings(|tap| feed_silence(tap, 1_024));
    let decoded = decode(&tile, analyzer.meta());
    assert!(
        decoded.iter().all(|db| *db == -100.0),
        "silence reads the floor"
    );
}

#[test]
fn json_levels_are_rounded_to_a_tenth_of_a_decibel() {
    let (reference, _, _) = both_encodings(|tap| {
        let mut phase = 0.0;
        feed_tone(tap, 3_000.0, 0.3, 1_024, &mut phase);
    });
    for db in reference {
        assert_eq!(db, (db * 10.0).round() / 10.0);
    }
}

/// The reason for the byte encoding: a default stream's tile must be a small
/// fraction of the size JSON numbers would make it.
#[test]
fn a_default_frame_is_compact_on_the_wire() {
    let tap = SpectrumTap::new();
    let mut analyzer =
        SpectrumAnalyzer::new(tap.reader(), RATE, "size", SpectrumConfig::default()).unwrap();
    let mut phase = 0.0;
    feed_tone(&tap, 1_000.0, 0.3, 1_024 + 512 * 7, &mut phase);
    let tile = analyzer.poll().unwrap();
    assert_eq!(tile.frame_count, 8);

    let bytes = serde_json::to_vec(&tile).unwrap().len();
    let per_frame = bytes / tile.frame_count as usize;
    // 513 levels are 684 base64 characters; allow a little for the envelope.
    assert!(per_frame < 720, "{per_frame} bytes a frame");
}

/// Audio mixed far past full scale is heard clipped, so the spectrum shows
/// the clipping's harmonics, and every level stays finite: numbers go out as
/// numbers, not as `null`, and parse back.
#[test]
fn an_overdriven_mix_reads_as_what_is_heard() {
    let (tap, mut analyzer) = analyzer(config());
    let bin = 8;
    let hz = bin as f32 * RATE as f32 / 256.0;
    let mut phase = 0.0;
    feed_tone(&tap, hz, 1.0e20, 1_024, &mut phase);
    let tile = analyzer.poll().unwrap();

    let levels = levels(&tile);
    assert!(levels.iter().all(|db| db.is_finite()));
    // A hard-clipped sine is nearly square: strong odd harmonics.
    assert!(
        levels[3 * bin] > -20.0,
        "the third harmonic reads {} dB",
        levels[3 * bin]
    );

    let event = crate::rpc::RpcEvent::new(crate::rpc::RpcEventPayload::SpectrogramTile(tile));
    let json = serde_json::to_string(&event).unwrap();
    assert!(!json.contains("null"));
    assert_eq!(
        serde_json::from_str::<crate::rpc::RpcEvent>(&json).unwrap(),
        event
    );
}
