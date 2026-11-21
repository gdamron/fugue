use super::*;
use crate::modules::sample_loading::{load_cached_sample, provide_package_sample};

/// A mono 16-bit WAV of `frames` frames at `rate`, a rising ramp so every
/// frame differs.
fn wav(frames: usize, rate: u32) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
    for frame in 0..frames {
        writer.write_sample((frame as i16 + 1) * 8).unwrap();
    }
    writer.finalize().unwrap();
    cursor.into_inner()
}

fn reference(text: &str) -> PackageAudioRef {
    PackageAudioRef::parse(text).unwrap()
}

// Each test names its own package: the provided files are process-wide.

#[test]
fn a_ref_resolves_to_the_highest_provided_version_meeting_its_requirement() {
    let id = "test.provided.versions";
    for version in ["0.1.0", "0.1.4", "0.2.0", "1.0.0"] {
        provide(id, version, "kick.wav", wav(8, 48_000)).unwrap();
    }
    provide(id, "0.1.9", "snare.wav", wav(8, 48_000)).unwrap();
    let resolved = |text: &str| resolve(&reference(text));
    assert_eq!(
        resolved(&format!("{id}@^0.1:kick.wav")).as_deref(),
        Some("test.provided.versions@=0.1.4:kick.wav")
    );
    // A bare version is a caret requirement, as in the package cache.
    assert_eq!(
        resolved(&format!("{id}@0.2.0:kick.wav")).as_deref(),
        Some("test.provided.versions@=0.2.0:kick.wav")
    );
    assert_eq!(
        resolved(&format!("{id}@=0.1.0:kick.wav")).as_deref(),
        Some("test.provided.versions@=0.1.0:kick.wav")
    );
    // The version must hold the file itself.
    assert_eq!(
        resolved(&format!("{id}@^0.1:snare.wav")).as_deref(),
        Some("test.provided.versions@=0.1.9:snare.wav")
    );
    assert_eq!(resolved(&format!("{id}@^2:kick.wav")), None);
    assert_eq!(resolved(&format!("{id}@^0.1:clap.wav")), None);
    assert_eq!(resolved("test.provided.absent@^1:kick.wav"), None);
}

#[test]
fn a_pinned_source_resolves_to_itself() {
    let id = "test.provided.pinned";
    let source = provide(id, "1.2.3", "dir/hat.wav", wav(8, 48_000)).unwrap();
    assert_eq!(source, "test.provided.pinned@=1.2.3:dir/hat.wav");
    assert_eq!(
        resolve(&reference(&source)).as_deref(),
        Some(source.as_str())
    );
    assert!(bytes(&source).is_some());
    assert!(bytes("test.provided.pinned@^1:dir/hat.wav").is_none());
    assert!(bytes("/tmp/dir/hat.wav").is_none());
}

#[test]
fn an_unresolved_ref_says_what_was_provided() {
    let none = unresolved(&reference("test.provided.nothing@^1:kick.wav"));
    assert!(
        none.contains("'test.provided.nothing@^1:kick.wav' was not provided")
            && none.contains("no file of package 'test.provided.nothing'"),
        "{none}"
    );
    let id = "test.provided.other";
    provide(id, "0.1.0", "kick.wav", wav(8, 48_000)).unwrap();
    let other = unresolved(&reference(&format!("{id}@^1:kick.wav")));
    assert!(
        other.contains("provided at 0.1.0") && other.contains("'^1'"),
        "{other}"
    );
}

#[test]
fn a_bad_name_or_a_changed_file_is_refused() {
    assert!(provide("bad id", "1.0.0", "kick.wav", wav(8, 48_000)).is_err());
    assert!(provide("test.provided.bad", "one", "kick.wav", wav(8, 48_000)).is_err());
    assert!(provide("test.provided.bad", "1.0.0", "../kick.wav", wav(8, 48_000)).is_err());
    assert!(provide("test.provided.bad", "1.0.0", "/kick.wav", wav(8, 48_000)).is_err());

    let id = "test.provided.changed";
    provide(id, "1.0.0", "kick.wav", wav(8, 48_000)).unwrap();
    provide(id, "1.0.0", "kick.wav", wav(8, 48_000)).expect("the same bytes again");
    let changed = provide(id, "1.0.0", "kick.wav", wav(9, 48_000)).unwrap_err();
    assert!(
        changed.contains("already provided with other bytes"),
        "{changed}"
    );
}

#[test]
fn a_provided_sample_decodes_at_the_engine_rate_through_the_cache() {
    let id = "test.provided.decoded";
    let source = provide_package_sample(id, "1.0.0", "ramp.wav", wav(480, 48_000), 48_000)
        .expect("a WAV decodes");
    let resolved = resolve(&reference(&format!("{id}@^1:ramp.wav"))).unwrap();
    assert_eq!(resolved, source);
    let sample = load_cached_sample(&resolved, 48_000).unwrap();
    assert_eq!(sample.len(), 480);
    // Mono is duplicated to both channels, scaled as a file load scales it.
    let (left, right) = sample.frame_at(9);
    assert_eq!(left, right);
    assert!((left - 80.0 / 32_768.0).abs() < 1e-6, "{left}");
    // The decode at the provided rate is the one the cache keeps.
    assert!(Arc::ptr_eq(
        &sample,
        &load_cached_sample(&resolved, 48_000).unwrap()
    ));
    // Another engine rate decodes the same bytes, resampled.
    assert_eq!(load_cached_sample(&resolved, 24_000).unwrap().len(), 240);
}

#[test]
fn bytes_that_do_not_decode_are_refused_and_not_kept() {
    let id = "test.provided.broken";
    let err =
        provide_package_sample(id, "1.0.0", "kick.wav", b"not audio".to_vec(), 48_000).unwrap_err();
    assert!(
        err.contains("sample test.provided.broken@=1.0.0:kick.wav did not load"),
        "{err}"
    );
    assert_eq!(resolve(&reference(&format!("{id}@^1:kick.wav"))), None);
    // Withdrawn, the file may be provided again with good bytes.
    provide_package_sample(id, "1.0.0", "kick.wav", wav(8, 48_000), 48_000).unwrap();
}

#[test]
fn a_provided_file_goes_through_format_detection() {
    let id = "test.provided.format";
    let err =
        provide_package_sample(id, "1.0.0", "kick.ogg", b"OggS....".to_vec(), 48_000).unwrap_err();
    assert!(err.contains("Unsupported audio format"), "{err}");
}

#[test]
fn providing_the_same_bytes_again_is_not_a_new_file() {
    // A decode that fails withdraws only a file it added, never one another
    // engine provided.
    let id = "test.provided.shared";
    let (source, new) = record(id, "1.0.0", "kick.wav", wav(8, 48_000)).unwrap();
    assert!(new);
    assert_eq!(
        record(id, "1.0.0", "kick.wav", wav(8, 48_000)).unwrap(),
        (source, false)
    );
}
