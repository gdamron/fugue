use super::*;

fn path(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

fn at(x: f64) -> Position {
    Position { x, y: 0.0 }
}

#[test]
fn a_key_joins_the_path_and_the_module_id() {
    assert_eq!(
        key(&path(&["voice"]), "filter").as_deref(),
        Some("voice/filter")
    );
    assert_eq!(
        key(&path(&["voice", "env"]), "attack").as_deref(),
        Some("voice/env/attack")
    );
}

#[test]
fn a_name_holding_the_separator_or_empty_has_no_key() {
    assert_eq!(key(&path(&["voice"]), "a/b"), None);
    assert_eq!(key(&path(&["vo/ice"]), "filter"), None);
    assert_eq!(key(&path(&["voice"]), ""), None);
}

#[test]
fn overrides_are_read_for_one_development_by_module_id() {
    let mut layout = Layout::default();
    layout.overrides.insert("voice/filter".into(), at(1.0));
    layout.overrides.insert("voice/env/attack".into(), at(2.0));
    layout.overrides.insert("lead/filter".into(), at(3.0));
    let voice = layout.overrides_in(&path(&["voice"]));
    assert_eq!(voice.len(), 1);
    assert_eq!(voice["filter"], at(1.0));
    let env = layout.overrides_in(&path(&["voice", "env"]));
    assert_eq!(env["attack"], at(2.0));
}

#[test]
fn layering_moves_only_modules_the_definition_has() {
    let definition = Invention::from_json(
        r#"{"modules":[{"id":"osc","type":"oscillator"},{"id":"vca","type":"vca"}],
            "connections":[],"layout":{"modules":{"osc":{"x":5,"y":5},"vca":{"x":9,"y":9}}}}"#,
    )
    .unwrap();
    assert!(layered(&definition, &BTreeMap::new()).is_none());
    let overrides = BTreeMap::from([("osc".to_string(), at(40.0)), ("ghost".into(), at(1.0))]);
    let shown = layered(&definition, &overrides).unwrap();
    assert_eq!(shown.layout.modules["osc"], at(40.0));
    assert_eq!(shown.layout.modules["vca"], Position { x: 9.0, y: 9.0 });
    assert!(!shown.layout.modules.contains_key("ghost"));
}

#[test]
fn a_layout_with_overrides_round_trips_and_adds_no_minimum_core() {
    let json = r#"{"modules":[{"id":"osc","type":"oscillator"}],"connections":[],
        "layout":{"overrides":{"voice/filter":{"x":12.5,"y":-3}}}}"#;
    let document = Invention::from_json(json).unwrap();
    assert_eq!(
        document.layout.overrides["voice/filter"],
        Position { x: 12.5, y: -3.0 }
    );
    assert!(!document.layout.is_empty());
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("piece.json");
    document.save_to_file(&file).unwrap();
    let saved = Invention::from_file(file.to_str().unwrap()).unwrap();
    assert_eq!(saved.layout, document.layout);
    assert_eq!(saved.minimum_core_version, None);
}

/// What lets an older core load a piece with overrides: a layout ignores a
/// member it does not know, as the core before overrides ignored this one.
#[test]
fn a_layout_member_this_core_does_not_know_is_ignored() {
    let json = r#"{"modules":[{"id":"osc","type":"oscillator"}],"connections":[],
        "layout":{"modules":{"osc":{"x":1,"y":2}},"from_a_later_core":{"a/b":{"x":1,"y":1}}}}"#;
    let document = Invention::from_json(json).unwrap();
    assert_eq!(document.layout.modules["osc"], Position { x: 1.0, y: 2.0 });
}
