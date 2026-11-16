//! Index entries for what the daemon declares: module types, content,
//! packages and the loaded piece's controls. Each is built from the reply a
//! discovery read already gives, never from a copy kept by hand.

use serde_json::{json, Map, Value};

use super::{Card, Entry, FindKind};
use crate::pkg::content::{ContentEntry, ContentKind, ContentPage};
use crate::rpc::{ModuleTypeDetail, ModuleTypeInfo, ModuleTypeList, PackageList};
use crate::{tidy_key, ControlKind, ControlMeta, ControlValue, RuntimeFullSnapshot};

/// The most of a type's controls its card's config fragment sets.
const FRAGMENT_CONTROLS: usize = 6;
/// The most control keys a module type's card names in its `when`.
const NAMED_CONTROLS: usize = 8;
/// The longest control value a card repeats; a control holding a longer one
/// gets the call that reads it.
const MAX_SHOWN_VALUE_CHARS: usize = 64;

/// A card per module type a full type listing describes. A type that
/// could not be inspected with its defaults still gets one, saying so.
pub fn module_type_entries(list: &ModuleTypeList) -> Vec<Entry> {
    (list.details.iter().flatten())
        .map(|detail| match detail {
            ModuleTypeDetail::Available { info } => module_type_entry(info),
            ModuleTypeDetail::Unavailable { type_name, error } => unread_type_entry(
                type_name,
                &format!(
                    "Needs a config to describe; with its defaults: {}",
                    error.message
                ),
            ),
        })
        .collect()
}

/// A card for a module type known only by name, saying why.
pub(super) fn unread_type_entry(type_name: &str, why: &str) -> Entry {
    Entry::new(Card {
        kind: FindKind::ModuleType,
        name: type_name.to_string(),
        what: format!("{} module", tidy_key(type_name)),
        when: why.to_string(),
        how: json!({
            "module": {"id": type_name, "type": type_name, "config": {}},
            "detail": {"tool": "describe_module", "arguments": {"type": type_name, "config": {}}},
        }),
    })
}

fn module_type_entry(info: &ModuleTypeInfo) -> Entry {
    let name = &info.type_name;
    let label = (info.label.clone()).unwrap_or_else(|| tidy_key(name));
    let mut when = format!("A {} module", label.to_lowercase());
    if info.is_sink {
        when.push_str(" that sends the piece's sound out");
    }
    when.push('.');
    let controls: Vec<&str> = (info.controls.iter())
        .filter(|meta| !meta.read_only)
        .map(|meta| meta.key.as_str())
        .collect();
    if !controls.is_empty() {
        let shown = controls.len().min(NAMED_CONTROLS);
        when.push_str(&format!(" Controls: {}", controls[..shown].join(", ")));
        if controls.len() > shown {
            when.push_str(&format!(" and {} more", controls.len() - shown));
        }
        when.push('.');
    }
    let config: Map<String, Value> = (info.controls.iter())
        .filter(|meta| !meta.read_only && !meta.event && !meta.key.contains('.'))
        .filter_map(|meta| Some((meta.key.clone(), shown_value(&meta.default)?)))
        .take(FRAGMENT_CONTROLS)
        .collect();
    let mut terms: Vec<String> = Vec::new();
    for meta in &info.controls {
        terms.extend(control_terms(meta));
    }
    terms.extend(info.inputs.iter().chain(&info.outputs).cloned());
    terms.extend(
        info.input_labels
            .values()
            .chain(info.output_labels.values())
            .cloned(),
    );
    Entry::new(Card {
        kind: FindKind::ModuleType,
        name: name.clone(),
        what: format!("{label} module"),
        when,
        how: json!({
            "module": {"id": name, "type": name, "config": config},
            "inputs": info.inputs,
            "outputs": info.outputs,
            "detail": {"tool": "describe_module", "arguments": {"type": name}},
        }),
    })
    .with_terms(terms)
    .with_text(info.controls.iter().map(|meta| meta.description.clone()))
}

/// A card per development or playable example in a content page.
pub fn content_entries(page: &ContentPage) -> Vec<Entry> {
    page.items.iter().map(content_entry).collect()
}

fn content_entry(item: &ContentEntry) -> Entry {
    let reference = serde_json::to_value(&item.reference).unwrap_or(Value::Null);
    let detail_arguments = json!({"schema_version": 1, "ref": reference});
    let (kind, how) = match item.kind {
        ContentKind::Development => {
            let alias = development_alias(&item.id);
            (
                FindKind::Development,
                json!({
                    "declare": {"name": alias, "ref": reference},
                    "module": {"id": alias, "type": alias},
                    "detail": {"tool": "describe_development", "arguments": detail_arguments},
                }),
            )
        }
        ContentKind::Invention => (
            FindKind::Example,
            json!({
                "call": {"tool": "load_invention", "arguments": {"ref": reference}},
                "detail": {"tool": "describe_example", "arguments": detail_arguments},
            }),
        ),
    };
    Entry::new(Card {
        kind,
        name: item.id.clone(),
        what: item.name.clone(),
        when: item.summary.clone(),
        how,
    })
    .with_terms([words_of(&item.source)])
}

/// The type name a piece might declare a development under: the last part
/// of its id, as a key (`fugue.fx.tape-delay` is `tape_delay`).
fn development_alias(id: &str) -> String {
    let last = id.rsplit(['.', '/']).next().unwrap_or(id);
    last.replace('-', "_")
}

/// A card per package in a package page. A kind this build does not know
/// is listed by name like any other.
pub fn package_entries(list: &PackageList) -> Vec<Entry> {
    (list.packages.iter())
        .map(|package| {
            let kind = package.kind.as_str();
            let mut how = json!({
                "call": {"tool": "list_packages", "arguments": {"id": package.id, "detail": true}},
            });
            if let Some(reference) = &package.content_ref {
                how["ref"] = serde_json::to_value(reference).unwrap_or(Value::Null);
            }
            Entry::new(Card {
                kind: FindKind::Package,
                name: package.id.clone(),
                what: format!("{} {} ({kind} package)", package.id, package.version),
                when: package.summary.clone(),
                how,
            })
            .with_terms([kind.to_string(), words_of(&package.source)])
            .with_terms(package.module_types.iter().cloned())
        })
        .collect()
}

/// A card per control of each module the loaded piece runs, with the call
/// that sets it (or reads it, when only its module writes it).
pub fn control_entries(snapshot: &RuntimeFullSnapshot) -> Vec<Entry> {
    let mut entries = Vec::new();
    for module in &snapshot.modules {
        let info = &module.info;
        let shown = info.name.clone().unwrap_or_else(|| info.id.clone());
        for control in &module.controls {
            let meta = &control.meta;
            let label = meta.label.clone().unwrap_or_else(|| tidy_key(&meta.key));
            let value = control.value.as_ref().unwrap_or(&meta.default);
            let mut arguments = json!({"module_id": info.id, "key": meta.key});
            let now = shown_value(value);
            // A value too long to repeat is read, never overwritten with a
            // stand-in.
            let tool = match (meta.read_only, meta.event, &now) {
                (false, true, _) => {
                    arguments["value"] = Value::Bool(true);
                    "set_control"
                }
                (false, false, Some(now)) => {
                    arguments["value"] = now.clone();
                    "set_control"
                }
                _ => "get_control",
            };
            let mut when = meta.description.trim_end().to_string();
            if !when.is_empty() && !when.ends_with(['.', '!', '?']) {
                when.push('.');
            }
            if let Some(now) = &now {
                when.push_str(&format!(" Now {now}"));
                when.push_str(&range_of(&meta.kind));
                when.push('.');
            } else if !meta.read_only && !meta.event {
                when.push_str(
                    " Its value is too long to show: read it with this call, then set it with set_control.",
                );
            }
            let mut terms = control_terms(meta);
            terms.extend([info.id.clone(), info.module_type.clone()]);
            terms.extend(info.name.clone());
            entries.push(
                Entry::new(Card {
                    kind: FindKind::Control,
                    name: format!("{}.{}", info.id, meta.key),
                    what: format!("{label} of {shown} ({})", info.module_type),
                    when,
                    how: json!({"call": {"tool": tool, "arguments": arguments}}),
                })
                .with_terms(terms),
            );
        }
    }
    entries
}

/// The words a control is found by: its key, label and choices.
fn control_terms(meta: &ControlMeta) -> Vec<String> {
    let mut terms = vec![meta.key.clone()];
    terms.extend(meta.label.clone());
    if let ControlKind::String {
        options: Some(options),
    } = &meta.kind
    {
        terms.extend(options.iter().cloned());
    }
    terms.extend(meta.option_labels.values().cloned());
    terms
}

/// A value short enough to repeat in a card.
fn shown_value(value: &ControlValue) -> Option<Value> {
    match value {
        ControlValue::String(text) if text.chars().count() > MAX_SHOWN_VALUE_CHARS => None,
        // As the f32 prints, so 0.1 is not 0.10000000149011612.
        ControlValue::Number(number) => (number.to_string().parse::<f64>().ok())
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number),
        value => serde_json::to_value(value).ok(),
    }
}

fn range_of(kind: &ControlKind) -> String {
    match kind {
        ControlKind::Number { min, max, .. } => format!(" (from {min} to {max})"),
        ControlKind::String {
            options: Some(options),
        } => format!(" (one of {})", options.join(", ")),
        _ => String::new(),
    }
}

/// How serde spells a unit enum value, as a word to search.
fn words_of(value: &impl serde::Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(word)) => word,
        _ => String::new(),
    }
}
