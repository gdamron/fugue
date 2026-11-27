//! The comparison itself, over two documents in the save format.

use super::{DiffEntry, PerformedValue, SoundingValue, Values};
use crate::rpc::inspection::fits;
use crate::rpc::MAX_INSPECTION_VALUE_BYTES;
use serde_json::{Map, Value};

/// The piece's fields compared by name, one entry each.
const NAMED: [(&str, &str); 1] = [("developments", "name")];

/// Every document change from `saved` to `current`, in document order.
pub(super) fn changes(saved: &Value, current: &Value) -> Vec<DiffEntry> {
    let mut changes = modules(list(saved, "modules"), list(current, "modules"));
    let (before, after) = (list(saved, "connections"), list(current, "connections"));
    let missing = |from: &[Value], wire: &Value| !from.iter().any(|other| same(other, wire));
    changes.extend(
        (before.iter().filter(|wire| missing(after, wire))).map(|wire| {
            DiffEntry::ConnectionRemoved {
                connection: wire.clone(),
            }
        }),
    );
    changes.extend(
        (after.iter().filter(|wire| missing(before, wire))).map(|wire| {
            DiffEntry::ConnectionAdded {
                connection: wire.clone(),
            }
        }),
    );
    changes.extend(piece(saved, current));
    changes
}

/// The values in `sounding` the current document does not hold, one per
/// control (the last given), in the order given.
pub(super) fn performed(current: &Value, sounding: &[SoundingValue]) -> Vec<PerformedValue> {
    let modules = list(current, "modules");
    let mut performed: Vec<PerformedValue> = Vec::new();
    for value in sounding {
        let document = (modules.iter())
            .find(|module| module.get("id").and_then(Value::as_str) == Some(&value.module_id))
            .and_then(|module| module.get("config")?.get(&value.key));
        performed.retain(|held| (held.module_id != value.module_id) || (held.key != value.key));
        if document.is_some_and(|held| sounds_as(held, &value.value)) {
            continue;
        }
        performed.push(PerformedValue {
            module_id: value.module_id.clone(),
            key: value.key.clone(),
            document: document.cloned(),
            sounding: value.value.clone(),
        });
    }
    performed
}

fn modules(saved: &[Value], current: &[Value]) -> Vec<DiffEntry> {
    let id = |module: &Value| {
        module
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let kind =
        |module: &Value| (module.get("type").and_then(Value::as_str).unwrap_or("")).to_string();
    let find =
        |from: &'_ [Value], wanted: &str| from.iter().find(|module| id(module) == wanted).cloned();
    let mut changes = Vec::new();
    for module in saved {
        if find(current, &id(module)).is_none() {
            changes.push(DiffEntry::ModuleRemoved {
                id: id(module),
                module_type: kind(module),
            });
        }
    }
    for module in current {
        let Some(before) = find(saved, &id(module)) else {
            changes.push(DiffEntry::ModuleAdded {
                id: id(module),
                module_type: kind(module),
            });
            continue;
        };
        let fields = keys(&before, module).filter(|field| !matches!(*field, "id" | "config"));
        for field in fields {
            if let Some(values) = differs(before.get(field), module.get(field)) {
                changes.push(DiffEntry::ModuleChanged {
                    id: id(module),
                    field: field.to_string(),
                    values,
                });
            }
        }
        let empty = Value::Object(Map::new());
        let config = |module: &Value| match module.get("config") {
            None | Some(Value::Null) => empty.clone(),
            Some(config) => config.clone(),
        };
        let (was, now) = (config(&before), config(module));
        if !(was.is_object() && now.is_object()) {
            if let Some(values) = differs(Some(&was), Some(&now)) {
                changes.push(DiffEntry::ModuleChanged {
                    id: id(module),
                    field: "config".to_string(),
                    values,
                });
            }
            continue;
        }
        for key in keys(&was, &now) {
            if let Some(values) = differs(was.get(key), now.get(key)) {
                changes.push(DiffEntry::ConfigChanged {
                    module_id: id(module),
                    key: key.to_string(),
                    values,
                });
            }
        }
    }
    changes
}

/// The piece's fields other than its modules and connections: each
/// development by name, each asset by its key, every other field whole.
fn piece(saved: &Value, current: &Value) -> Vec<DiffEntry> {
    let mut changes = Vec::new();
    let fields = keys(saved, current).filter(|field| !matches!(*field, "modules" | "connections"));
    for field in fields {
        let (before, after) = (saved.get(field), current.get(field));
        let named = NAMED
            .iter()
            .find(|(name, _)| *name == field)
            .map(|(_, key)| *key);
        let entries: Option<Named> = match (named, field) {
            (Some(key), _) => Some(by_name(before, after, key)),
            (None, "assets") => Some(by_key(before, after)),
            _ => None,
        };
        let Some(entries) = entries else {
            if let Some(values) = differs(before, after) {
                changes.push(DiffEntry::PieceChanged {
                    field: field.to_string(),
                    name: None,
                    values,
                });
            }
            continue;
        };
        for (name, before, after) in entries {
            if let Some(values) = differs(before, after) {
                changes.push(DiffEntry::PieceChanged {
                    field: field.to_string(),
                    name: Some(name),
                    values,
                });
            }
        }
    }
    changes
}

type Named<'a> = Vec<(String, Option<&'a Value>, Option<&'a Value>)>;

/// The entries of two lists of objects, matched by their `key` field.
fn by_name<'a>(before: Option<&'a Value>, after: Option<&'a Value>, key: &str) -> Named<'a> {
    let items = |list: Option<&'a Value>| {
        list.and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
    };
    let name = |item: &Value| (item.get(key).and_then(Value::as_str).unwrap_or("")).to_string();
    let mut names: Vec<String> = items(before).iter().map(name).collect();
    for item in items(after) {
        if !names.contains(&name(item)) {
            names.push(name(item));
        }
    }
    let find = |list, wanted: &str| items(list).iter().find(|item| name(item) == wanted);
    (names.into_iter())
        .map(|wanted| {
            let (was, now) = (find(before, &wanted), find(after, &wanted));
            (wanted, was, now)
        })
        .collect()
}

/// The entries of two objects, matched by key.
fn by_key<'a>(before: Option<&'a Value>, after: Option<&'a Value>) -> Named<'a> {
    let empty = Value::Null;
    (keys(before.unwrap_or(&empty), after.unwrap_or(&empty)))
        .map(|key| {
            let (was, now) = (
                before.and_then(|v| v.get(key)),
                after.and_then(|v| v.get(key)),
            );
            (key.to_string(), was, now)
        })
        .collect()
}

/// The keys of two objects, those of `a` first, each once.
fn keys<'a>(a: &'a Value, b: &'a Value) -> impl Iterator<Item = &'a str> {
    let first = a.as_object().into_iter().flat_map(Map::keys);
    let second = (b.as_object().into_iter().flat_map(Map::keys))
        .filter(move |key| a.get(key.as_str()).is_none());
    first.chain(second).map(String::as_str)
}

fn list<'a>(document: &'a Value, field: &str) -> &'a [Value] {
    document
        .get(field)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// The two sides of a change, or `None` when they are the same.
fn differs(saved: Option<&Value>, current: Option<&Value>) -> Option<Values> {
    let same = match (saved, current) {
        (Some(a), Some(b)) => same(a, b),
        (None, None) => true,
        _ => false,
    };
    if same {
        return None;
    }
    let small =
        |value: Option<&Value>| value.is_none_or(|value| fits(value, MAX_INSPECTION_VALUE_BYTES));
    if small(saved) && small(current) {
        return Some(Values {
            saved: saved.cloned(),
            current: current.cloned(),
            omitted: false,
        });
    }
    Some(Values {
        omitted: true,
        ..Values::default()
    })
}

/// Whether a control sounds as the document holds it: a control's number
/// is an `f32`, so the document's number is read as one.
fn sounds_as(held: &Value, sounding: &Value) -> bool {
    match (held.as_f64(), sounding.as_f64()) {
        (Some(held), Some(sounding)) => held as f32 == sounding as f32,
        _ => same(held, sounding),
    }
}

/// Whether two values are the same, a number by its value: a document
/// that holds `440` and one that holds `440.0` say the same.
pub(super) fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a == b || (a.as_f64() == b.as_f64()),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, a)| b.get(key).is_some_and(|b| same(a, b)))
        }
        _ => a == b,
    }
}
