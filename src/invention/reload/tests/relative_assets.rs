//! A live write of a relative path to an asset control loads the file the
//! saved document loads: it resolves against the invention's directory, as
//! the loader resolves the document's, for every module that takes one.

use serde_json::json;

use super::sample_levels::{constant_wav, document, peak, start_pumped};
use super::*;
use crate::rpc::StructuralEdit;

/// Each module that takes an asset: its config playing `asset`, and the
/// control that re-points it.
fn modules(asset: &str) -> [(serde_json::Value, &'static str); 3] {
    [
        (
            json!({ "type": "sample_player", "config": { "asset": asset } }),
            "asset",
        ),
        (
            json!({ "type": "sample_kit", "config": { "samples": [{ "key": 36, "asset": asset }] } }),
            "asset.0",
        ),
        (
            json!({ "type": "sample_instrument", "config": { "zones": [{ "root_note": 60, "asset": asset }] } }),
            "asset.0",
        ),
    ]
}

/// `module` as `m` in an invention saved at `dir/piece.json`, loaded from it.
fn from_file(dir: &std::path::Path, module: serde_json::Value) -> Invention {
    let path = dir.join("piece.json");
    document(module).save_to_file(&path).unwrap();
    Invention::from_file(&path.to_string_lossy()).unwrap()
}

/// Lays out `dir/piece.wav` and `dir/sub/written.wav`: the second only under
/// the invention's directory, not the process's working directory.
fn layout(dir: &std::path::Path) -> (String, String) {
    std::fs::create_dir(dir.join("sub")).unwrap();
    let original = constant_wav(dir, 0.5, 4_800);
    let written = constant_wav(&dir.join("sub"), 0.25, 4_800);
    assert!(written.ends_with("sub/level-0.25.wav"));
    assert!(!std::path::Path::new("sub/level-0.25.wav").exists());
    (original, "sub/level-0.25.wav".to_string())
}

#[test]
fn a_live_relative_write_loads_the_file_the_saved_document_loads() {
    let dir = tempfile::tempdir().unwrap();
    let (original, relative) = layout(dir.path());
    for (module, key) in modules(&original) {
        let (running, _pump) = start_pumped(from_file(dir.path(), module));
        running
            .set_control("m", key, ControlValue::String(relative.clone()))
            .unwrap_or_else(|error| panic!("{key}: {error}"));
        // The document keeps the path as written.
        let saved = running.document().unwrap();
        let config = &saved.modules[0].config;
        let recorded = match config.get("samples").or(config.get("zones")) {
            Some(entries) => &entries[0]["asset"],
            None => &config["asset"],
        };
        assert_eq!(recorded, &json!(relative), "{config}");

        let path = dir.path().join("piece.json");
        saved.save_to_file(&path).unwrap();
        let (loaded, _) = start_pumped(Invention::from_file(&path.to_string_lossy()).unwrap());
        let live = running.get_control("m", key).unwrap();
        assert_eq!(live, loaded.get_control("m", key).unwrap(), "{key}");
        assert_eq!(
            live,
            ControlValue::String(dir.path().join(&relative).to_string_lossy().into_owned())
        );

        // The stored config names the same file, so reloading the saved
        // document is no change.
        let mut running = running;
        let report = running.reload(Invention::from_file(&path.to_string_lossy()).unwrap());
        assert!(report.unwrap().swapped.is_empty(), "{key}");
    }
}

#[test]
fn a_relative_write_plays_the_file_beside_the_invention() {
    let dir = tempfile::tempdir().unwrap();
    let (original, relative) = layout(dir.path());
    let module = json!({ "type": "sample_kit", "config": { "samples": [{ "key": 36, "asset": original }] } });
    let (running, pump) = start_pumped(from_file(dir.path(), module));
    running
        .set_control("m", "asset.0", ControlValue::String(relative))
        .unwrap();
    assert!((peak(&running, &pump, ("play", 36.0)) - 0.25).abs() < 1e-3);
}

#[test]
fn an_edit_batch_resolves_a_relative_write_the_same_way() {
    let dir = tempfile::tempdir().unwrap();
    let (original, relative) = layout(dir.path());
    for (module, key) in modules(&original) {
        let (mut running, _pump) = start_pumped(from_file(dir.path(), module));
        let report = running
            .apply_edits(&[StructuralEdit::SetControl {
                module_id: "m".into(),
                key: key.into(),
                value: ControlValue::String(relative.clone()),
            }])
            .unwrap_or_else(|error| panic!("{key}: {error:?}"));
        assert!(report.controls_failed.is_empty(), "{key}: {report:?}");
        let expected = dir.path().join(&relative).to_string_lossy().into_owned();
        assert_eq!(
            running.get_control("m", key).unwrap(),
            ControlValue::String(expected)
        );
    }
}

#[test]
fn saving_elsewhere_rebases_a_relative_asset() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let (original, relative) = layout(dir.path());
    for (module, key) in modules(&original) {
        let (running, _pump) = start_pumped(from_file(dir.path(), module));
        running
            .set_control("m", key, ControlValue::String(relative.clone()))
            .unwrap();
        let path = elsewhere.path().join("copy.json");
        running.document().unwrap().save_to_file(&path).unwrap();
        let (loaded, _) = start_pumped(Invention::from_file(&path.to_string_lossy()).unwrap());
        assert_eq!(
            running.get_control("m", key).unwrap(),
            loaded.get_control("m", key).unwrap(),
            "{key}"
        );
    }
}

#[test]
fn a_relative_write_without_an_invention_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (original, relative) = layout(dir.path());
    for (module, key) in modules(&original) {
        let (mut running, _pump) = start_pumped(document(module));
        let before = running.document().unwrap().to_json().unwrap();
        let error = running
            .set_control("m", key, ControlValue::String(relative.clone()))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("has no file to resolve it against"),
            "{error}"
        );
        assert!(error.contains("give an absolute path"), "{error}");
        let refused = running.apply_edits(&[StructuralEdit::SetControl {
            module_id: "m".into(),
            key: key.into(),
            value: ControlValue::String(relative.clone()),
        }]);
        let error = format!("{:?}", refused.unwrap_err());
        assert!(
            error.contains("has no file to resolve it against"),
            "{error}"
        );
        assert_eq!(
            running.document().unwrap().to_json().unwrap(),
            before,
            "{key}"
        );
        assert_eq!(
            running.get_control("m", key).unwrap(),
            ControlValue::String(original.clone())
        );

        // An absolute path still loads.
        let absolute = dir.path().join(&relative).to_string_lossy().into_owned();
        running
            .set_control("m", key, ControlValue::String(absolute))
            .unwrap();
    }
}

#[test]
fn package_refs_urls_and_rooted_paths_pass_through() {
    use crate::invention::audio_assets::resolve_asset_write;
    let source = std::path::Path::new("/song/piece.json");
    for text in [
        "fugue.drums.808@1.2.0:kick.wav",
        "https://example.com/a.wav",
        "/abs/a.wav",
    ] {
        let value = ControlValue::String(text.into());
        assert_eq!(
            resolve_asset_write(Some(source), "asset", &value).unwrap(),
            value
        );
        assert_eq!(resolve_asset_write(None, "asset.2", &value).unwrap(), value);
    }
    // Only asset controls resolve.
    let value = ControlValue::String("a.wav".into());
    assert_eq!(resolve_asset_write(None, "pattern", &value).unwrap(), value);
    assert_eq!(resolve_asset_write(None, "asset.x", &value).unwrap(), value);
}
