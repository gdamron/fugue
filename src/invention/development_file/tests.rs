use super::splice::with_layout;
use super::*;
use crate::invention::layout::Position;

fn layout(modules: &[(&str, f64, f64)]) -> Layout {
    let mut layout = Layout::default();
    for (id, x, y) in modules {
        layout
            .modules
            .insert(id.to_string(), Position { x: *x, y: *y });
    }
    layout
}

const PLACED: &str = r#"{
  "version": "1.0.0",
  "modules": [{ "id": "osc", "type": "oscillator" }],
  "connections": [],
  "layout": { "modules": { "osc": { "x": 1.0, "y": 2.0 } } },
  "title": "keeps its place"
}
"#;

const UNPLACED: &str = "{\n\t\"version\": \"1.0.0\",\n\t\"modules\": [{ \"id\": \"osc\", \"type\": \"oscillator\" }],\n\t\"connections\": []\n}\n";

fn parsed(text: &str) -> Invention {
    Invention::from_json(text).expect("a document")
}

#[test]
fn a_layout_is_replaced_in_place_leaving_every_other_byte() {
    let moved = layout(&[("osc", 30.0, 40.0)]);
    let written = with_layout(PLACED, &[], &moved).unwrap();
    let start = PLACED.find("\"layout\"").unwrap();
    let after = PLACED.find(",\n  \"title\"").unwrap();
    assert_eq!(&written[..start], &PLACED[..start]);
    assert!(written.ends_with(&PLACED[after..]));
    assert_eq!(parsed(&written).layout, moved);
}

#[test]
fn a_layout_is_added_after_the_last_member_with_its_indentation() {
    let moved = layout(&[("osc", 5.0, 6.0)]);
    let written = with_layout(UNPLACED, &[], &moved).unwrap();
    let last = UNPLACED.find("[]").unwrap() + 2;
    assert_eq!(&written[..last], &UNPLACED[..last]);
    assert!(written.contains("\n\t\"layout\": {\n\t  \"modules\""));
    assert!(written.ends_with("\n}\n"));
    assert_eq!(parsed(&written).layout, moved);
}

#[test]
fn an_empty_layout_removes_the_member_and_its_comma() {
    let written = with_layout(PLACED, &[], &Layout::default()).unwrap();
    assert!(!written.contains("layout"));
    let mut expected = parsed(PLACED);
    expected.layout = Layout::default();
    assert_eq!(parsed(&written), expected);
    assert_eq!(
        with_layout(UNPLACED, &[], &Layout::default()).unwrap(),
        UNPLACED
    );
}

#[test]
fn a_layout_first_or_alone_is_removed_cleanly() {
    let first = r#"{"layout": {"modules": {}}, "version": "1.0.0", "modules": []}"#;
    let written = with_layout(first, &[], &Layout::default()).unwrap();
    assert_eq!(written, r#"{"version": "1.0.0", "modules": []}"#);
}

#[test]
fn an_inline_definition_is_found_by_its_chain_and_strings_are_read_whole() {
    let text = r#"{
  "version": "1.0.0",
  "modules": [],
  "connections": [],
  "developments": [
    { "name": "a \"quoted\" }", "definition": { "version": "1.0.0", "modules": [], "connections": [] } },
    { "name": "outer", "definition": {
      "version": "1.0.0",
      "modules": [{ "id": "osc", "type": "oscillator" }],
      "connections": [],
      "developments": [{ "name": "inner", "definition": { "version": "1.0.0", "modules": [{ "id": "env", "type": "adsr" }], "connections": [] } }]
    } }
  ]
}"#;
    let moved = layout(&[("env", 7.0, 8.0)]);
    let within = ["outer".to_string(), "inner".to_string()];
    let written = with_layout(text, &within, &moved).unwrap();
    let mut expected = parsed(text);
    inline_definition_mut(&mut expected, &within)
        .unwrap()
        .layout = moved;
    assert_eq!(parsed(&written), expected);
    let missing = ["outer".to_string(), "nowhere".to_string()];
    assert_eq!(with_layout(text, &missing, &layout(&[])), None);
}

fn file(text: &str) -> (tempfile::TempDir, PathBuf, Invention) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("voice.json");
    fs::write(&path, text).unwrap();
    let loaded = Invention::from_file(path.to_str().unwrap()).unwrap();
    (dir, path, loaded)
}

#[cfg(unix)]
#[test]
fn a_write_changes_only_the_layout_and_keeps_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, path, loaded) = file(UNPLACED);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let moved = layout(&[("osc", 1.0, 1.0)]);
    write_layout(&path, &[], &loaded, &moved).unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o640);
    let mut expected = parsed(UNPLACED);
    expected.layout = moved;
    assert_eq!(parsed(&fs::read_to_string(&path).unwrap()), expected);
    let leftovers = fs::read_dir(path.parent().unwrap()).unwrap().count();
    assert_eq!(leftovers, 1, "nothing is left beside the file");
}

#[test]
fn a_file_changed_since_it_loaded_is_not_written() {
    let (_dir, path, loaded) = file(PLACED);
    let changed = PLACED.replace("keeps its place", "edited elsewhere");
    fs::write(&path, &changed).unwrap();
    let refused = write_layout(&path, &[], &loaded, &layout(&[("osc", 9.0, 9.0)]));
    assert!(matches!(refused, Err(NotWritten::Failed(ref why)) if why.contains("changed on disk")));
    assert_eq!(fs::read_to_string(&path).unwrap(), changed);
}

#[cfg(unix)]
#[test]
fn a_file_that_cannot_be_written_is_refused_untouched() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, path, loaded) = file(PLACED);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    if fs::OpenOptions::new().write(true).open(&path).is_ok() {
        return; // run as a user who writes anything
    }
    let refused = write_layout(&path, &[], &loaded, &layout(&[("osc", 9.0, 9.0)]));
    assert!(
        matches!(refused, Err(NotWritten::Failed(ref why)) if why.contains("cannot be written"))
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), PLACED);
}

#[test]
fn a_file_saved_by_another_writer_before_the_rename_is_not_replaced() {
    let (dir, path, _) = file(PLACED);
    fs::write(&path, "saved meanwhile").unwrap();
    assert!(!replace(&path, PLACED, "arranged").unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), "saved meanwhile");
    assert_eq!(
        fs::read_dir(dir.path()).unwrap().count(),
        1,
        "nothing left beside it"
    );
}

#[test]
fn replacements_of_one_file_never_share_a_staging_file() {
    let (dir, path, _) = file(PLACED);
    let first = beside(&path);
    assert_ne!(first, beside(&path));
    // A staging file already there (another replacement's) is left alone.
    let taken = beside(&path);
    fs::write(&taken, "another replacement").unwrap();
    assert!(replace(&path, PLACED, "arranged").unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), "arranged");
    assert_eq!(fs::read_to_string(&taken).unwrap(), "another replacement");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
}
