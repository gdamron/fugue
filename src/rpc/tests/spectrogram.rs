//! Wire tests for spectrogram streams.

use super::*;

fn meta(encoding: SpectrogramEncoding) -> SpectrogramStreamMeta {
    SpectrogramStreamMeta {
        stream_id: "master:1".to_string(),
        provenance: SpectrogramProvenance {
            sample_rate: 48_000,
            fft_size: 1024,
            hop_size: 512,
            window: SpectrogramWindow::Hann,
            source: "sink:master".to_string(),
        },
        frequency: SpectrogramFrequencyAxis {
            spacing: SpectrogramBinSpacing::Linear,
            min_hz: 0.0,
            bin_hz: 46.875,
            bin_count: 513,
        },
        db: SpectrogramDbScale {
            reference: SpectrogramDbReference::Dbfs,
            floor_db: -100.0,
            ceiling_db: 0.0,
        },
        limits: SpectrogramLimits {
            history_frames: 512,
            max_frames_per_tile: 8,
        },
        encoding,
    }
}

#[test]
fn stream_announcement_round_trips() {
    let event = RpcEvent::new(RpcEventPayload::SpectrogramStream(meta(
        SpectrogramEncoding::U8Base64,
    )));
    let json = serde_json::to_value(&event).unwrap();

    assert_eq!(json["event"], "spectrogram_stream");
    assert_eq!(json["schema_version"], RPC_SCHEMA_VERSION);
    assert_eq!(json["stream_id"], "master:1");
    assert_eq!(json["provenance"]["window"], "hann");
    assert_eq!(json["provenance"]["source"], "sink:master");
    assert_eq!(json["frequency"]["spacing"], "linear");
    assert_eq!(json["frequency"]["bin_count"], 513);
    assert_eq!(json["db"]["reference"], "dbfs");
    assert_eq!(json["limits"]["max_frames_per_tile"], 8);
    assert_eq!(json["encoding"], "u8_base64");

    assert_eq!(serde_json::from_value::<RpcEvent>(json).unwrap(), event);
}

#[test]
fn encodings_name_themselves_on_the_wire() {
    for (encoding, name) in [
        (SpectrogramEncoding::U8Base64, "u8_base64"),
        (SpectrogramEncoding::F32Json, "f32_json"),
    ] {
        assert_eq!(serde_json::to_value(encoding).unwrap(), name);
    }
}

#[test]
fn tiles_round_trip_in_either_encoding() {
    for magnitudes in [
        SpectrogramMagnitudes::Numbers(vec![-98.4, -91.2, -72.5, -88.0]),
        SpectrogramMagnitudes::Text("BBkxzw==".to_string()),
    ] {
        let tile = SpectrogramTile {
            stream_id: "master:1".to_string(),
            tile_seq: 412,
            start_frame: 1173,
            frame_count: 2,
            magnitudes: magnitudes.clone(),
        };
        let event = RpcEvent::new(RpcEventPayload::SpectrogramTile(tile));
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(json["event"], "spectrogram_tile");
        assert_eq!(json["tile_seq"], 412);
        assert_eq!(json["start_frame"], 1173);
        assert_eq!(json["frame_count"], 2);
        match &magnitudes {
            SpectrogramMagnitudes::Numbers(_) => assert!(json["magnitudes"].is_array()),
            SpectrogramMagnitudes::Text(_) => assert!(json["magnitudes"].is_string()),
        }
        assert_eq!(serde_json::from_value::<RpcEvent>(json).unwrap(), event);
    }
}

#[test]
fn a_tile_must_match_the_shape_and_encoding_its_stream_declared() {
    let mut json_meta = meta(SpectrogramEncoding::F32Json);
    json_meta.frequency.bin_count = 2;
    let mut base64_meta = json_meta.clone();
    base64_meta.encoding = SpectrogramEncoding::U8Base64;

    let good = SpectrogramTile {
        stream_id: "master:1".to_string(),
        tile_seq: 0,
        start_frame: 0,
        frame_count: 2,
        magnitudes: SpectrogramMagnitudes::Numbers(vec![-100.0, -90.0, -80.0, -70.0]),
    };
    assert!(good.matches(&json_meta));
    assert!(
        !good.matches(&base64_meta),
        "numbers on a stream that declared base64"
    );

    // Four bytes take eight base64 characters.
    let packed = SpectrogramTile {
        magnitudes: SpectrogramMagnitudes::Text("AAECAw==".to_string()),
        ..good.clone()
    };
    assert!(packed.matches(&base64_meta));
    let short_packed = SpectrogramTile {
        magnitudes: SpectrogramMagnitudes::Text("AAEC".to_string()),
        ..good.clone()
    };
    assert!(!short_packed.matches(&base64_meta));

    let wrong_stream = SpectrogramTile {
        stream_id: "master:2".to_string(),
        ..good.clone()
    };
    assert!(!wrong_stream.matches(&json_meta));

    let wrong_length = SpectrogramTile {
        magnitudes: SpectrogramMagnitudes::Numbers(vec![-100.0; 3]),
        ..good.clone()
    };
    assert!(!wrong_length.matches(&json_meta));

    let too_many_frames = SpectrogramTile {
        frame_count: json_meta.limits.max_frames_per_tile + 1,
        ..good.clone()
    };
    assert!(!too_many_frames.matches(&json_meta));

    let empty = SpectrogramTile {
        frame_count: 0,
        magnitudes: SpectrogramMagnitudes::Numbers(Vec::new()),
        ..good
    };
    assert!(!empty.matches(&json_meta));
}

/// Length alone cannot tell four bytes from five: both take eight
/// characters. Padding can.
#[test]
fn base64_padding_fixes_the_byte_count() {
    let mut meta = meta(SpectrogramEncoding::U8Base64);
    meta.frequency.bin_count = 2;
    let tile = |text: &str| SpectrogramTile {
        stream_id: "master:1".to_string(),
        tile_seq: 0,
        start_frame: 0,
        frame_count: 2,
        magnitudes: SpectrogramMagnitudes::Text(text.to_string()),
    };
    assert!(tile("AAECAw==").matches(&meta), "four bytes");
    assert!(!tile("AAECAwQ=").matches(&meta), "five bytes");
    assert!(!tile("AAECAwQF").matches(&meta), "six bytes");
}

/// Encodings and spacings from a newer producer still parse, so a client can
/// report the stream unsupported instead of failing on the announcement.
#[test]
fn unknown_encodings_and_spacings_parse_as_unsupported() {
    let mut json = serde_json::to_value(meta(SpectrogramEncoding::U8Base64)).unwrap();
    json["encoding"] = "zstd_f16".into();
    json["frequency"]["spacing"] = "mel".into();
    let parsed: SpectrogramStreamMeta = serde_json::from_value(json).unwrap();
    assert_eq!(parsed.encoding, SpectrogramEncoding::Unsupported);
    assert_eq!(parsed.frequency.spacing, SpectrogramBinSpacing::Unsupported);

    let tile = SpectrogramTile {
        stream_id: "master:1".to_string(),
        tile_seq: 0,
        start_frame: 0,
        frame_count: 1,
        magnitudes: SpectrogramMagnitudes::Text("AAAA".to_string()),
    };
    assert!(
        !tile.matches(&parsed),
        "nothing matches an unknown encoding"
    );
}

/// A malformed tile or stream must be rejected, never overflow into a panic.
#[test]
fn absurd_shapes_are_rejected_without_overflow() {
    let mut meta = meta(SpectrogramEncoding::U8Base64);
    meta.frequency.bin_count = u32::MAX;
    meta.limits.max_frames_per_tile = u32::MAX;
    let tile = SpectrogramTile {
        stream_id: "master:1".to_string(),
        tile_seq: 0,
        start_frame: 0,
        frame_count: u32::MAX,
        magnitudes: SpectrogramMagnitudes::Text(String::new()),
    };
    assert!(!tile.matches(&meta));
}

#[test]
fn topic_names_itself_on_the_wire() {
    let json = serde_json::to_value(RpcSubscriptionTopic::Spectrograms).unwrap();
    assert_eq!(json, "spectrograms");
    assert_eq!(
        serde_json::from_value::<RpcSubscriptionTopic>(json).unwrap(),
        RpcSubscriptionTopic::Spectrograms
    );
}
