//! A development key reaching a sample instrument's note off is reserved
//! as the note off is, so a live write to it takes the room a live graph
//! keeps for reserved requests; a key reaching a note on is not.

use std::collections::HashSet;

use serde_json::json;

use super::*;
use crate::modules::SampleInstrumentFactory;
use crate::test_support::sampler::wav;

/// One instrument, `p`, played through `note` and released through `off`.
fn factory(a: &str) -> DevelopmentFactory {
    let mut registry = ModuleRegistry::default();
    registry.register(SampleInstrumentFactory);
    let zone = json!({ "zones": [{ "root_note": 60, "key_range": [0, 127], "asset": a }] });
    let definition = json!({
        "version": "1.0.0",
        "modules": [{ "id": "p", "type": "sample_instrument", "config": zone }],
        "connections": [],
        "outputs": [{ "name": "p", "from": "p", "from_port": "audio_left" }],
        "controls": [
            { "name": "note", "module": "p", "control": "note_on" },
            { "name": "off", "module": "p", "control": "note_off" }
        ]
    });
    DevelopmentFactory {
        name: "solo".to_string(),
        definition: serde_json::from_value(definition).unwrap(),
        registry,
        registered: Arc::new(Mutex::new(HashSet::new())),
        loaded: Arc::new(LoadedDevelopments::default()),
    }
}

#[test]
fn a_key_reaching_a_note_off_is_reserved_as_the_note_off_is() {
    let dir = tempfile::tempdir().unwrap();
    let a = wav(dir.path(), "a.wav", 441);
    let built = factory(&a).build(48_000, &json!({})).unwrap();
    let (table, _) = built.module.module().declared().unwrap();
    let reserved = |key| {
        let (decl, _) = table.decl(table.resolve(key).unwrap()).unwrap();
        (decl.event, decl.reserved)
    };
    assert_eq!(reserved("off"), (true, true));
    assert_eq!(reserved("note"), (true, false));
}
