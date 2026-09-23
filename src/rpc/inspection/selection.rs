use super::{
    entry, invalid, InspectionCoverage as Coverage, InspectionEntry, InspectionSelection, RpcError,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(super) fn select(
    root: &Value,
    selection: &InspectionSelection,
) -> Result<Vec<InspectionEntry>, RpcError> {
    let mut out = Vec::new();
    match selection {
        InspectionSelection::Value { pointer } => {
            let value = root
                .pointer(pointer)
                .ok_or_else(|| invalid("authored pointer not found"))?;
            match value {
                Value::Object(object) => {
                    for (key, value) in object {
                        out.push(entry(child(pointer, key), value, Coverage::Complete));
                    }
                }
                Value::Array(array) => {
                    for (index, value) in array.iter().enumerate() {
                        out.push(entry(
                            child(pointer, &index.to_string()),
                            value,
                            Coverage::Complete,
                        ));
                    }
                }
                Value::String(text) if !super::fits(value, super::MAX_INSPECTION_VALUE_BYTES) => {
                    let mut start = 0;
                    while start < text.len() {
                        // Even six-byte JSON escapes fit the individual value budget.
                        let mut end = (start + 512).min(text.len());
                        while !text.is_char_boundary(end) {
                            end -= 1;
                        }
                        out.push(InspectionEntry {
                            pointer: pointer.clone(),
                            coverage: Coverage::Fragment,
                            value: Some(Value::String(text[start..end].into())),
                            string_range: Some([start, end]),
                        });
                        start = end;
                    }
                }
                _ => out.push(entry(pointer.clone(), value, Coverage::Complete)),
            }
        }
        InspectionSelection::Overview { scope } => {
            overview(document(root, scope)?, scope, &mut out);
        }
        InspectionSelection::Module { scope, id } => {
            let doc = document(root, scope)?;
            let (index, module) = array(doc, "modules")
                .iter()
                .enumerate()
                .find(|(_, m)| m["id"] == *id)
                .ok_or_else(|| invalid("module not found in authored scope; use overview"))?;
            out.push(entry(
                format!("{scope}/modules/{index}"),
                module,
                Coverage::Complete,
            ));
            wiring(doc, scope, &BTreeSet::from([id.as_str()]), &mut out);
            if let Some(name) = module["type"].as_str() {
                development_reference(root, scope, name, &mut out);
            }
            let mut assets = BTreeSet::new();
            asset_names(&module["config"], &mut assets);
            asset_entries(doc, scope, assets, &mut out);
        }
        InspectionSelection::Development { scope, name } => {
            let doc = document(root, scope)?;
            let (index, development) = array(doc, "developments")
                .iter()
                .enumerate()
                .find(|(_, d)| d["name"] == *name)
                .ok_or_else(|| invalid("development not found in authored scope; use overview"))?;
            out.push(development_entry(scope, index, development));
            let mut ids = BTreeSet::new();
            let mut assets = BTreeSet::new();
            for (index, module) in array(doc, "modules").iter().enumerate() {
                if module["type"] == *name {
                    asset_names(&module["config"], &mut assets);
                    if let Some(id) = module["id"].as_str() {
                        ids.insert(id);
                    }
                    out.push(entry(
                        format!("{scope}/modules/{index}"),
                        module,
                        Coverage::Complete,
                    ));
                }
            }
            wiring(doc, scope, &ids, &mut out);
            asset_entries(doc, scope, assets, &mut out);
            if development["definition"].is_object() {
                overview(
                    &development["definition"],
                    &format!("{scope}/developments/{index}/definition"),
                    &mut out,
                );
            }
        }
    }
    Ok(out)
}

fn document<'a>(root: &'a Value, scope: &str) -> Result<&'a Value, RpcError> {
    // Only retained inline definitions are document scopes. No filesystem reads.
    let mut doc = root;
    if !scope.is_empty() {
        let parts: Vec<_> = scope.strip_prefix('/').unwrap_or("").split('/').collect();
        if parts.len() % 3 != 0 {
            return Err(invalid(
                "scope must point to an inline development definition",
            ));
        }
        for part in parts.chunks(3) {
            if part[0] != "developments" || part[2] != "definition" {
                return Err(invalid(
                    "scope must point to an inline development definition",
                ));
            }
            let index = part[1]
                .parse::<usize>()
                .map_err(|_| invalid("invalid development index"))?;
            if index.to_string() != part[1] {
                return Err(invalid(
                    "development index must use canonical decimal notation",
                ));
            }
            doc = doc.get("developments").and_then(|v| v.get(index)).and_then(|v| v.get("definition"))
                .filter(|v| v.is_object()).ok_or_else(|| invalid("inline development scope not found; external declarations require describe_development or source access"))?;
        }
    }
    Ok(doc)
}

fn overview(doc: &Value, scope: &str, out: &mut Vec<InspectionEntry>) {
    for field in ["version", "title", "description"] {
        out.push(entry(child(scope, field), &doc[field], Coverage::Complete));
    }
    for (index, module) in array(doc, "modules").iter().enumerate() {
        out.push(module_entry(scope, index, module));
    }
    for (index, development) in array(doc, "developments").iter().enumerate() {
        out.push(development_entry(scope, index, development));
    }
    // Summarize section sizes; inspect each section with Value when needed.
    for field in ["connections", "assets", "inputs", "outputs", "controls"] {
        let value = &doc[field];
        let count = value
            .as_array()
            .map(Vec::len)
            .or_else(|| value.as_object().map(|o| o.len()))
            .unwrap_or(0);
        out.push(entry(
            child(scope, field),
            &json!({"count":count}),
            Coverage::Summary,
        ));
    }
}

fn wiring(doc: &Value, scope: &str, ids: &BTreeSet<&str>, out: &mut Vec<InspectionEntry>) {
    let mut peers = BTreeSet::new();
    for (index, connection) in array(doc, "connections").iter().enumerate() {
        let from = connection["from"].as_str().unwrap_or("");
        let to = connection["to"].as_str().unwrap_or("");
        if ids.contains(from) || ids.contains(to) {
            out.push(entry(
                format!("{scope}/connections/{index}"),
                connection,
                Coverage::Complete,
            ));
            peers.extend([from, to]);
        }
    }
    for (index, module) in array(doc, "modules").iter().enumerate() {
        let id = module["id"].as_str().unwrap_or("");
        if peers.contains(id) && !ids.contains(id) {
            out.push(module_entry(scope, index, module));
        }
    }
    for (field, target) in [
        ("inputs", "to"),
        ("outputs", "from"),
        ("controls", "module"),
    ] {
        for (index, alias) in array(doc, field).iter().enumerate() {
            if ids.contains(alias[target].as_str().unwrap_or("")) {
                out.push(entry(
                    format!("{scope}/{field}/{index}"),
                    alias,
                    Coverage::Complete,
                ));
            }
        }
    }
}

fn module_entry(scope: &str, index: usize, module: &Value) -> InspectionEntry {
    entry(
        format!("{scope}/modules/{index}"),
        &json!({"id":module["id"],"type":module["type"]}),
        Coverage::Summary,
    )
}

fn development_entry(scope: &str, index: usize, development: &Value) -> InspectionEntry {
    let pointer = format!("{scope}/developments/{index}");
    if development["definition"].is_object() {
        entry(
            pointer,
            &json!({"name":development["name"],"path":development["path"],"ref":development["ref"],"definition_pointer":format!("{scope}/developments/{index}/definition")}),
            Coverage::Summary,
        )
    } else {
        entry(pointer, development, Coverage::Complete)
    }
}

fn development_reference(root: &Value, scope: &str, name: &str, out: &mut Vec<InspectionEntry>) {
    // Nested factories inherit the registry as it existed when their declaration
    // was registered: only earlier declarations in each ancestor are visible.
    // Registration skips aliases already inherited, so an outer match wins
    // over the same name declared locally (even across multiple ancestors).
    let mut selected = None;
    let mut scope = scope;
    let mut before = usize::MAX;
    loop {
        let doc = root.pointer(scope).expect("validated authored scope");
        if let Some((index, development)) = array(doc, "developments")
            .iter()
            .enumerate()
            .take(before)
            .find(|(_, d)| d["name"] == name)
        {
            selected = Some((scope, index, development));
        }
        let Some((parent, tail)) = scope.rsplit_once("/developments/") else {
            break;
        };
        before = tail
            .split('/')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        scope = parent;
    }
    if let Some((scope, index, development)) = selected {
        out.push(development_entry(scope, index, development));
    }
}

fn asset_entries(
    doc: &Value,
    scope: &str,
    names: BTreeSet<String>,
    out: &mut Vec<InspectionEntry>,
) {
    for name in names {
        if let Some(asset) = doc["assets"].get(&name) {
            out.push(entry(
                child(&format!("{scope}/assets"), &name),
                asset,
                Coverage::Complete,
            ));
        }
    }
}

fn asset_names(value: &Value, names: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            if let Some(name) = object.get("$asset").and_then(Value::as_str) {
                names.insert(name.to_owned());
            }
            for value in object.values() {
                asset_names(value, names);
            }
        }
        Value::Array(array) => {
            for value in array {
                asset_names(value, names);
            }
        }
        _ => {}
    }
}

fn array<'a>(doc: &'a Value, field: &str) -> &'a [Value] {
    doc[field].as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn child(pointer: &str, key: &str) -> String {
    format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1"))
}
