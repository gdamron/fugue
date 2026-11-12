use super::*;

const NAMED: &str = r#"{
    "modules": [
        { "id": "tone", "type": "filter", "name": "Lead filter" },
        { "id": "osc", "type": "oscillator" }
    ],
    "connections": []
}"#;

fn name<'a>(document: &'a Invention, id: &str) -> Option<&'a str> {
    name_of(document, id)
}

#[test]
fn a_name_loads_and_saves_and_an_unnamed_module_saves_without_one() {
    let document = Invention::from_json(NAMED).unwrap();
    assert_eq!(name(&document, "tone"), Some("Lead filter"));
    assert_eq!(name(&document, "osc"), None);
    let saved = document.to_json().unwrap();
    assert_eq!(saved.matches("\"name\"").count(), 1, "{saved}");
    let again = Invention::from_json(&saved).unwrap();
    assert_eq!(again, document);
}

#[test]
fn a_document_without_names_saves_without_any() {
    let json = r#"{"modules":[{"id":"osc","type":"oscillator"}],"connections":[]}"#;
    let saved = Invention::from_json(json).unwrap().to_json().unwrap();
    assert!(!saved.contains("\"name\""), "{saved}");
}

#[test]
fn names_raise_no_minimum_core() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("named.json");
    Invention::from_json(NAMED)
        .unwrap()
        .save_to_file(&path)
        .unwrap();
    let loaded = Invention::from_file(path.to_str().unwrap()).unwrap();
    assert_eq!(loaded.minimum_core_version, None);
    assert_eq!(name(&loaded, "tone"), Some("Lead filter"));
}

#[test]
fn a_name_is_not_part_of_what_a_document_builds() {
    let named = Invention::from_json(NAMED).unwrap();
    let mut plain = named.clone();
    plain.modules[0].name = None;
    assert_ne!(named, plain);
    assert_eq!(named.as_built(), plain.as_built());
}

#[test]
fn names_that_cannot_name_a_module_are_refused() {
    assert_eq!(name_refusal("Lead filter"), None);
    assert_eq!(name_refusal("Bass · 2"), None);
    assert_eq!(name_refusal(&"é".repeat(MAX_MODULE_NAME_BYTES / 2)), None);
    for bad in [
        "",
        "   ",
        "Lead\nfilter",
        "tab\there",
        "Lead\u{2028}filter",
        "Lead\u{202E}filter",
        "Lead\u{200B}filter",
    ] {
        assert!(name_refusal(bad).is_some(), "{bad:?}");
    }
    assert!(name_refusal(&"x".repeat(MAX_MODULE_NAME_BYTES + 1)).is_some());
}

#[test]
fn settling_drops_names_that_cannot_be_used_with_a_warning() {
    let mut document = Invention::from_json(NAMED).unwrap();
    document.modules[1].name = Some("  ".into());
    let dropped = document.settle_presentation();
    assert_eq!(dropped.len(), 1, "{dropped:?}");
    assert!(dropped[0].contains("'osc'"), "{dropped:?}");
    assert_eq!(name(&document, "osc"), None);
    assert_eq!(name(&document, "tone"), Some("Lead filter"));
}

#[test]
fn changes_between_documents_list_each_name_that_differs() {
    let base = Invention::from_json(NAMED).unwrap();
    let mut next = base.clone();
    next.modules[0].name = None;
    next.modules[1].name = Some("Saw".into());
    let changes = PresentationChanges::between(Some(&base), &next);
    assert_eq!(
        changes.names,
        [
            ("tone".to_string(), None),
            ("osc".to_string(), Some("Saw".to_string()))
        ]
    );
    assert!(PresentationChanges::between(Some(&base), &base)
        .names
        .is_empty());
    let from_nothing = PresentationChanges::between(None, &base);
    assert_eq!(
        from_nothing.names,
        [("tone".to_string(), Some("Lead filter".to_string()))]
    );
}

#[test]
fn carrying_keeps_names_given_since_and_makes_the_changes_on_top() {
    // The batch read NAMED, renamed osc, and its document is adopted after
    // tone was renamed meanwhile.
    let mut batch = Invention::from_json(NAMED).unwrap();
    batch.modules[1].name = Some("Saw".into());
    let changes = PresentationChanges {
        layout: Vec::new(),
        names: vec![("osc".into(), Some("Saw".into()))],
    };
    let mut current = Invention::from_json(NAMED).unwrap();
    current.modules[0].name = Some("Bright filter".into());

    batch.carry_presentation(&current, &changes);

    assert_eq!(name(&batch, "tone"), Some("Bright filter"));
    assert_eq!(name(&batch, "osc"), Some("Saw"));
}
