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
        SpectrogramMagnitudes::F32Json(vec![-98.4, -91.2, -72.5, -88.0]),
        SpectrogramMagnitudes::U8Base64("BBkxzw==".to_string()),
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
            SpectrogramMagnitudes::F32Json(_) => assert!(json["magnitudes"].is_array()),
            SpectrogramMagnitudes::U8Base64(_) => assert!(json["magnitudes"].is_string()),
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
        magnitudes: SpectrogramMagnitudes::F32Json(vec![-100.0, -90.0, -80.0, -70.0]),
    };
    assert!(good.matches(&json_meta));
    assert!(
        !good.matches(&base64_meta),
        "numbers on a stream that declared base64"
    );

    // Four bytes take eight base64 characters.
    let packed = SpectrogramTile {
        magnitudes: SpectrogramMagnitudes::U8Base64("AAECAw==".to_string()),
        ..good.clone()
    };
    assert!(packed.matches(&base64_meta));
    let short_packed = SpectrogramTile {
        magnitudes: SpectrogramMagnitudes::U8Base64("AAEC".to_string()),
        ..good.clone()
    };
    assert!(!short_packed.matches(&base64_meta));

    let wrong_stream = SpectrogramTile {
        stream_id: "master:2".to_string(),
        ..good.clone()
    };
    assert!(!wrong_stream.matches(&json_meta));

    let wrong_length = SpectrogramTile {
        magnitudes: SpectrogramMagnitudes::F32Json(vec![-100.0; 3]),
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
        magnitudes: SpectrogramMagnitudes::F32Json(Vec::new()),
        ..good
    };
    assert!(!empty.matches(&json_meta));
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
