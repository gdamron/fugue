use super::*;

fn spectrogram_meta() -> SpectrogramStreamMeta {
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
        encoding: SpectrogramEncoding::F32Json,
    }
}

#[test]
fn spectrogram_stream_announcement_round_trips() {
    let event = RpcEvent::new(RpcEventPayload::SpectrogramStream(spectrogram_meta()));
    let json = serde_json::to_value(&event).unwrap();

    assert_eq!(json["event"], "spectrogram_stream");
    assert_eq!(json["schema_version"], RPC_SCHEMA_VERSION);
    assert_eq!(json["stream_id"], "master:1");
    assert_eq!(json["provenance"]["window"], "hann");
    assert_eq!(json["provenance"]["source"], "sink:master");
    assert_eq!(json["frequency"]["bin_count"], 513);
    assert_eq!(json["db"]["reference"], "dbfs");
    assert_eq!(json["limits"]["max_frames_per_tile"], 8);
    // Named from the first stream on, so a compact encoding can be added
    // later without a schema version bump.
    assert_eq!(json["encoding"], "f32_json");

    assert_eq!(serde_json::from_value::<RpcEvent>(json).unwrap(), event);
}

#[test]
fn spectrogram_tile_round_trips() {
    let tile = SpectrogramTile {
        stream_id: "master:1".to_string(),
        tile_seq: 412,
        start_frame: 1173,
        frame_count: 2,
        magnitudes_db: vec![-98.4, -91.2, -72.5, -88.0],
    };
    let event = RpcEvent::new(RpcEventPayload::SpectrogramTile(tile.clone()));
    let json = serde_json::to_value(&event).unwrap();

    assert_eq!(json["event"], "spectrogram_tile");
    assert_eq!(json["tile_seq"], 412);
    assert_eq!(json["start_frame"], 1173);
    assert_eq!(json["frame_count"], 2);
    assert_eq!(json["magnitudes_db"].as_array().unwrap().len(), 4);

    assert_eq!(serde_json::from_value::<RpcEvent>(json).unwrap(), event);
}

#[test]
fn a_tile_must_match_the_shape_its_stream_declared() {
    let meta = SpectrogramStreamMeta {
        frequency: SpectrogramFrequencyAxis {
            bin_count: 2,
            ..spectrogram_meta().frequency
        },
        ..spectrogram_meta()
    };
    let good = SpectrogramTile {
        stream_id: "master:1".to_string(),
        tile_seq: 0,
        start_frame: 0,
        frame_count: 2,
        magnitudes_db: vec![-100.0, -90.0, -80.0, -70.0],
    };
    assert!(good.matches(&meta));

    let wrong_stream = SpectrogramTile {
        stream_id: "master:2".to_string(),
        ..good.clone()
    };
    assert!(!wrong_stream.matches(&meta));

    let wrong_length = SpectrogramTile {
        magnitudes_db: vec![-100.0; 3],
        ..good.clone()
    };
    assert!(!wrong_length.matches(&meta));

    let too_many_frames = SpectrogramTile {
        frame_count: meta.limits.max_frames_per_tile + 1,
        ..good.clone()
    };
    assert!(!too_many_frames.matches(&meta));

    let empty = SpectrogramTile {
        frame_count: 0,
        magnitudes_db: Vec::new(),
        ..good
    };
    assert!(!empty.matches(&meta));
}

#[test]
fn spectrogram_topic_names_itself_on_the_wire() {
    let json = serde_json::to_value(RpcSubscriptionTopic::Spectrograms).unwrap();
    assert_eq!(json, "spectrograms");
    assert_eq!(
        serde_json::from_value::<RpcSubscriptionTopic>(json).unwrap(),
        RpcSubscriptionTopic::Spectrograms
    );
}
