//! An offline write that loses to a higher-ranked one at the same sample
//! changes none of a development's aliases: the declared ones keep the
//! winner, and so do the legacy ones the same key reaches.

use serde_json::json;

use super::declared_tests::{factory, mixed, outputs_a, running_from};
use super::DevelopmentFactory;
use crate::control_request::Source;
use crate::invention::declared::WriteOptions;

/// A user's write of `mix`, then a script's at the same sample, which
/// loses: the dial and the lfo both keep the user's.
fn a_losing_write_leaves_every_alias(factory: DevelopmentFactory) {
    let (graph, surface) = running_from(factory, json!({ "mix": 0.5 }));
    let write = |value: f32, source| {
        let options = WriteOptions::performed(source);
        surface.submit("mix", value.into(), &options).unwrap()
    };
    assert!(!write(0.75, Source::User).lost);
    assert!(write(0.25, Source::Script).lost, "the user's holds");
    assert_eq!(outputs_a(&graph), 0.75, "the dial");
    assert_eq!(surface.get_control("rate").unwrap(), 0.75.into(), "the lfo");
}

#[test]
fn a_losing_offline_write_leaves_a_mixed_keys_legacy_alias() {
    a_losing_write_leaves_every_alias(factory("mixed", mixed()));
}

#[test]
fn a_losing_offline_write_leaves_a_nested_mixed_keys_legacy_alias() {
    let outer = json!({
        "version": "1.0.0",
        "developments": [{ "name": "inner", "definition": mixed() }],
        "modules": [{ "id": "n", "type": "inner" }],
        "connections": [],
        "outputs": [{ "name": "a", "from": "n", "from_port": "a" }],
        "controls": [
            { "name": "mix", "module": "n", "control": "mix" },
            { "name": "rate", "module": "n", "control": "rate" }
        ]
    });
    a_losing_write_leaves_every_alias(factory("outer", outer));
}
