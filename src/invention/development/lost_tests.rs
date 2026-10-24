//! An offline write that loses to a higher-ranked one at the same sample
//! changes none of a development's aliases: the declared ones keep the
//! winner, and so do the legacy ones the same key reaches, whether it loses
//! as it is ranked or, overtaken by a replay after that, as it is applied.

use serde_json::json;

use super::declared_tests::{factory, mixed, outputs_a, running_from};
use super::DevelopmentFactory;
use crate::control_request::Source;
use crate::invention::declared::{DeclaredSurface, WriteOptions};

/// A user's write of `mix`, then a script's at the same sample, which
/// loses (as it is applied, when `overtaken`): the dial and the legacy
/// dial both keep the user's.
fn a_losing_write_leaves_every_alias(factory: DevelopmentFactory, overtaken: bool) {
    let (graph, surface) = running_from(factory, json!({ "mix": 0.5 }));
    let write = |value: f32, source| {
        let options = WriteOptions::performed(source);
        surface.submit("mix", value.into(), &options).unwrap()
    };
    assert!(!write(0.75, Source::User).lost);
    let lost = match overtaken {
        true => DeclaredSurface::admitting_all(|| write(0.25, Source::Script)),
        false => write(0.25, Source::Script),
    };
    assert!(lost.lost, "the user's holds");
    assert_eq!(outputs_a(&graph), 0.75, "the dial");
    assert_eq!(surface.get_control("rate").unwrap(), 0.75.into(), "the legacy dial");
}

#[test]
fn a_losing_offline_write_leaves_a_mixed_keys_legacy_alias() {
    for overtaken in [false, true] {
        a_losing_write_leaves_every_alias(factory("mixed", mixed()), overtaken);
    }
}

#[test]
fn a_losing_offline_write_leaves_a_nested_mixed_keys_legacy_alias() {
    for overtaken in [false, true] {
        a_losing_write_leaves_every_alias(factory("outer", nested()), overtaken);
    }
}

/// `mixed()` inside another development exposing its `mix` and `rate`.
fn nested() -> serde_json::Value {
    json!({
        "version": "1.0.0",
        "developments": [{ "name": "inner", "definition": mixed() }],
        "modules": [{ "id": "n", "type": "inner" }],
        "connections": [],
        "outputs": [{ "name": "a", "from": "n", "from_port": "a" }],
        "controls": [
            { "name": "mix", "module": "n", "control": "mix" },
            { "name": "rate", "module": "n", "control": "rate" }
        ]
    })
}
