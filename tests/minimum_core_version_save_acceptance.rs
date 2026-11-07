//! Acceptance tests for the minimum core a saved document declares.
//!
//! A document that uses a feature only newer cores understand is saved with
//! a `minimum_core_version` no older than the core that introduced the
//! feature, so an older core refuses it instead of misreading it. Saving
//! never lowers a minimum the document already declares, and a document that
//! uses nothing versioned and declares nothing is saved without the field.
//!
//! The versioned feature under test is a development control's `range`,
//! which first requires core 2026.11.0.

use std::path::Path;

use fugue::{AudioBackend, Invention, InventionBuilder};
use serde_json::{json, Value};

mod minimum_core_version_support;
use minimum_core_version_support::*;

/// Saves `doc` as the core would for a session and reads the file back.
fn saved(doc: &Value) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("saved.json");
    parse(doc).save_to_file(&path).expect("saves");
    read(&path)
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).expect("saved JSON")
}

/// The minimum a saved document declares at its top level.
fn minimum_of(doc: &Value) -> Option<(u64, u64, u64)> {
    // The core writes unset optional fields as null; that is no minimum.
    let declared = doc.get(FIELD).filter(|v| !v.is_null())?;
    let text = declared
        .as_str()
        .unwrap_or_else(|| panic!("a string, got {declared}"));
    Some(version(text).unwrap_or_else(|| panic!("{text} is not YYYY.M.P")))
}

fn assert_at_least(saved: &Value, floor: (u64, u64, u64), label: &str) {
    match minimum_of(saved) {
        Some(minimum) => assert!(
            minimum >= floor,
            "{label}: saved minimum {minimum:?} is below {floor:?}"
        ),
        None => panic!("{label}: no {FIELD} was written\n{saved:#}"),
    }
}

// -- 4. control ranges raise the minimum ---------------------------------------------

#[test]
fn an_inline_development_using_a_control_range_saves_a_minimum_of_its_release() {
    let doc = kit_document(kit_definition(true));
    assert_at_least(&saved(&doc), RANGES_SINCE, "inline definition with a range");
}

#[test]
fn a_development_document_using_a_control_range_saves_a_minimum_of_its_release() {
    let doc = kit_definition(true);
    assert_at_least(
        &saved(&doc),
        RANGES_SINCE,
        "development document with a range",
    );
}

#[test]
fn a_range_raises_an_older_declared_minimum() {
    for minimum in ["2026.1.0", "2026.10.0"] {
        let doc = kit_document(declaring(kit_definition(true), minimum));
        assert_at_least(
            &saved(&doc),
            RANGES_SINCE,
            &format!("inline, declared {minimum}"),
        );

        let doc = declaring(kit_definition(true), minimum);
        assert_at_least(
            &saved(&doc),
            RANGES_SINCE,
            &format!("document, declared {minimum}"),
        );

        let doc = declaring(kit_document(kit_definition(true)), minimum);
        assert_at_least(
            &saved(&doc),
            RANGES_SINCE,
            &format!("parent, declared {minimum}"),
        );
    }
}

#[test]
fn a_saved_document_using_a_range_loads_again() {
    for doc in [kit_document(kit_definition(true)), kit_definition(true)] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved.json");
        parse(&doc).save_to_file(&path).unwrap();
        Invention::from_file(path.to_str().unwrap())
            .unwrap_or_else(|e| panic!("the saved file loads on the core that saved it: {e}"));
    }
}

#[test]
fn a_live_session_using_a_range_saves_a_minimum_of_its_release() {
    struct Silent;
    impl AudioBackend for Silent {
        fn sample_rate(&self) -> u32 {
            48_000
        }
        fn start(
            &mut self,
            _: Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>,
        ) -> Result<(), Box<dyn std::error::Error>> {
            Ok(())
        }
        fn stop(&mut self) {}
    }

    let (runtime, _) = InventionBuilder::new(48_000)
        .build(parse(&kit_document(kit_definition(true))))
        .expect("builds");
    let running = runtime.start_with_backend(Silent).expect("starts");
    let document = running
        .document_to_save()
        .expect("the document is retained");
    running.stop();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("saved.json");
    document.save_to_file(&path).expect("saves");
    assert_at_least(&read(&path), RANGES_SINCE, "session document");
}

// -- a saved minimum is never lowered ------------------------------------------------

#[test]
fn saving_keeps_a_declared_minimum_that_is_the_running_core() {
    let floor = running();
    let cases = [
        declaring(plain(), RUNNING),
        declaring(kit_document(kit_definition(false)), RUNNING),
        declaring(kit_document(kit_definition(true)), RUNNING),
        declaring(kit_definition(true), RUNNING),
    ];
    for doc in cases {
        assert_at_least(&saved(&doc), floor, "declared running core kept");
    }
}

#[test]
fn saving_keeps_a_declared_minimum_without_a_versioned_feature() {
    for minimum in ["2026.1.0", "2026.9.0"] {
        let doc = declaring(plain(), minimum);
        assert_at_least(&saved(&doc), version(minimum).unwrap(), "plain, declared");
    }
}

#[test]
fn saving_never_lowers_an_inline_definitions_declared_minimum() {
    let doc = kit_document(declaring(kit_definition(false), RUNNING));
    let saved = saved(&doc);
    assert_at_least(
        &saved["developments"][0]["definition"],
        running(),
        "inline definition's own minimum",
    );
}

// -- nothing versioned, nothing written -----------------------------------------------

#[test]
fn a_document_using_nothing_versioned_is_saved_without_the_field() {
    let cases = [
        ("plain", plain()),
        ("development document", kit_definition(false)),
        ("inline development", kit_document(kit_definition(false))),
    ];
    for (label, doc) in cases {
        let saved = saved(&doc);
        assert_eq!(
            minimum_of(&saved),
            None,
            "{label}: a minimum was written\n{saved:#}"
        );
        if let Some(definition) = saved["developments"].get(0).map(|d| &d["definition"]) {
            assert_eq!(
                minimum_of(definition),
                None,
                "{label}: the definition got a minimum"
            );
        }
    }
}

#[test]
fn saving_changes_nothing_else_in_the_document() {
    for doc in [
        kit_document(kit_definition(true)),
        kit_definition(true),
        plain(),
    ] {
        assert_eq!(
            without_field(saved(&doc)),
            without_field(serde_json::from_str(&parse(&doc).to_json().unwrap()).unwrap()),
            "saving rewrote more than the field"
        );
    }
}

#[test]
fn saved_range_keeps_its_bounds() {
    let saved = saved(&kit_document(kit_definition(true)));
    assert_eq!(
        saved["developments"][0]["definition"]["controls"][0]["range"],
        json!({ "min": 0.5, "max": 8.0 })
    );
}
