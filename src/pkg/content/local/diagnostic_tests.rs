//! Catalog diagnostics name the entry and the file that failed, so a broken
//! shared dependency does not read the same for every entry that uses it.

use super::*;
use serde_json::json;

fn roots() -> (tempfile::TempDir, ContentRoots) {
    let temp = tempfile::tempdir().unwrap();
    let roots = ContentRoots {
        packages: temp.path().join("packs"),
        workspace: temp.path().join("inventions"),
    };
    (temp, roots)
}

fn write(path: &Path, value: serde_json::Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

#[test]
fn a_broken_dependency_names_the_entry_and_the_file() {
    let (_temp, roots) = roots();
    // A development whose exposed control is spelled with the old `key`.
    write(
        &roots.workspace.join("lib").join("voice.json"),
        json!({
            "modules": [{"id": "osc", "type": "oscillator", "config": {}}],
            "connections": [],
            "controls": [{"key": "pitch", "module": "osc", "control": "frequency"}]
        }),
    );
    write(
        &roots.workspace.join("song.json"),
        json!({
            "developments": [{"name": "voice", "path": "lib/voice.json"}],
            "modules": [{"id": "lead", "type": "voice", "config": {}}],
            "connections": []
        }),
    );

    let page = ContentCatalog::new(roots, "session")
        .list(&ContentListQuery::default())
        .unwrap();

    let song = page
        .diagnostics
        .iter()
        .find(|d| d.message.starts_with("workspace song.json: "))
        .unwrap_or_else(|| panic!("no diagnostic for song.json: {:?}", page.diagnostics));
    assert_eq!(song.code, "invalid_content");
    assert!(song.message.contains("voice.json: "), "{song}");
    assert!(song.message.contains("missing field `name`"), "{song}");
}
