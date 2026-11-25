use serde_json::{json, Value};

use super::*;
use crate::find::{Card, FindKind, FindQuery};
use crate::pkg::content::{ContentEntry, ContentKind, ContentPage, ContentRef, ContentSource};
use crate::rpc::{
    DescribeModuleQuery, ModuleDescription, ModuleTypeList, PackageInfo, PackageList,
    RegistryScope, RpcError, RpcErrorCode, RuntimeControlSnapshot, RuntimeFullSnapshot,
    RuntimeModuleSnapshot, RuntimePortInfo,
};
use crate::{ModuleRegistry, RuntimeModuleInfo, RuntimeStatus};

const SAMPLE_RATE: u32 = 48_000;

/// A running module built as the daemon would describe it.
fn module(
    registry: &ModuleRegistry,
    id: &str,
    module_type: &str,
    name: Option<&str>,
) -> RuntimeModuleSnapshot {
    let query = DescribeModuleQuery {
        module_type: Some(module_type.into()),
        ..Default::default()
    };
    let described =
        ModuleDescription::from_registry(registry, SAMPLE_RATE, RegistryScope::Running, &query)
            .unwrap();
    RuntimeModuleSnapshot {
        info: RuntimeModuleInfo {
            id: id.into(),
            module_type: module_type.into(),
            config: json!({}),
            name: name.map(str::to_string),
        },
        ports: RuntimePortInfo {
            inputs: described.info.inputs.clone(),
            outputs: described.info.outputs.clone(),
        },
        controls: (described.info.controls.iter())
            .map(|meta| RuntimeControlSnapshot {
                meta: meta.clone(),
                value: described.control_values.get(&meta.key).cloned(),
            })
            .collect(),
        timeline: None,
    }
}

fn snapshot(running: bool, modules: Vec<RuntimeModuleSnapshot>) -> RuntimeFullSnapshot {
    RuntimeFullSnapshot {
        status: RuntimeStatus {
            running,
            paused: false,
            sample_rate: SAMPLE_RATE,
            module_count: modules.len(),
            connection_count: 0,
            diagnostics: None,
            session: None,
        },
        modules,
        connections: Vec::new(),
    }
}

fn content(id: &str, kind: ContentKind, summary: &str) -> ContentEntry {
    ContentEntry {
        id: id.into(),
        version: "1.0.0".into(),
        kind,
        name: id.rsplit('.').next().unwrap().into(),
        summary: summary.into(),
        source: ContentSource::Bundled,
        reference: ContentRef::Package {
            package: id.into(),
            version: "1.0.0".into(),
        },
    }
}

fn page(items: Vec<ContentEntry>, next_cursor: Option<&str>) -> ContentPage {
    ContentPage {
        schema_version: 1,
        generation: "g".into(),
        items,
        next_cursor: next_cursor.map(str::to_string),
        diagnostics: Vec::new(),
        diagnostics_truncated: false,
    }
}

/// A daemon answering every read the gather makes from `registry` and the
/// pieces given, with the example catalog split over two pages and the
/// development catalog failing.
struct Daemon {
    registry: ModuleRegistry,
    piece: RuntimeFullSnapshot,
    requests: Vec<Value>,
    /// Refuses a type description of more types than this, as a reply over
    /// the response cap is refused, and any naming `refused_type`.
    max_described: usize,
    refused_type: &'static str,
}

impl Daemon {
    fn reply(&mut self, request: RpcRequestPayload) -> Result<RpcResponsePayload, String> {
        self.requests.push(serde_json::to_value(&request).unwrap());
        let command = match request {
            RpcRequestPayload::GetSnapshot => {
                return Ok(RpcResponsePayload::Snapshot(self.piece.clone()))
            }
            RpcRequestPayload::Command(command) => command,
            other => panic!("unexpected request {other:?}"),
        };
        Ok(match command {
            RpcCommand::DescribeModuleTypes(query)
                if (query.types.iter()).any(|types| {
                    types.len() > self.max_described
                        || types.iter().any(|name| name == self.refused_type)
                }) =>
            {
                RpcResponsePayload::Error(RpcError::new(RpcErrorCode::Internal, "too large"))
            }
            RpcCommand::DescribeModuleTypes(query) => match ModuleTypeList::from_registry(
                &self.registry,
                SAMPLE_RATE,
                RegistryScope::Builtins,
                &query,
            ) {
                Ok(discovery) => RpcResponsePayload::ModuleTypes { discovery },
                Err(error) => RpcResponsePayload::Error(error),
            },
            RpcCommand::ListExamples { query } => RpcResponsePayload::Examples {
                catalog: match query.cursor.as_deref() {
                    None => page(
                        vec![content(
                            "fugue.demo.drone",
                            ContentKind::Invention,
                            "A slow drone.",
                        )],
                        Some("second"),
                    ),
                    Some(_) => page(
                        vec![content(
                            "fugue.demo.bwv772",
                            ContentKind::Invention,
                            "Bach's Invention 1.",
                        )],
                        None,
                    ),
                },
            },
            RpcCommand::ListDevelopments { .. } => {
                RpcResponsePayload::Error(RpcError::new(RpcErrorCode::Internal, "disk unreadable"))
            }
            RpcCommand::ListPackages { .. } => RpcResponsePayload::Packages(PackageList {
                packages: vec![
                    PackageInfo::built_in(&self.registry).compact(),
                    serde_json::from_value(json!({
                        "id": "fugue.fx.tape-delay", "version": "2026.9.0", "kind": "development",
                        "source": "installed", "summary": "A warm tape echo.",
                        "ref": {"package": "fugue.fx.tape-delay", "version": "2026.9.0"},
                    }))
                    .unwrap(),
                ],
                next_cursor: None,
                diagnostics: vec![serde_json::from_value(
                    json!({"code": "invalid_manifest", "message": "bad manifest"}),
                )
                .unwrap()],
                diagnostics_truncated: false,
            }),
            other => panic!("unexpected command {other:?}"),
        })
    }
}

fn gather(piece: RuntimeFullSnapshot) -> (Index, Vec<Value>) {
    gather_refusing(piece, usize::MAX, "")
}

fn gather_refusing(
    piece: RuntimeFullSnapshot,
    max_described: usize,
    refused_type: &'static str,
) -> (Index, Vec<Value>) {
    let mut daemon = Daemon {
        registry: ModuleRegistry::default(),
        piece,
        requests: Vec::new(),
        max_described,
        refused_type,
    };
    let mut gather = Gather::new();
    while let Some(request) = gather.next_request() {
        let reply = daemon.reply(request);
        gather.answer(reply);
    }
    (gather.finish(), daemon.requests)
}

fn card<'a>(index: &'a Index, kind: FindKind, name: &str) -> &'a Card {
    let entry = index
        .entries()
        .iter()
        .find(|e| e.card.kind == kind && e.card.name == name);
    &entry.unwrap_or_else(|| panic!("no {kind:?} {name}")).card
}

#[test]
fn every_registered_module_type_gets_a_card() {
    let (index, requests) = gather(snapshot(false, Vec::new()));
    for name in ModuleRegistry::default().types() {
        assert!(
            index.contains(FindKind::ModuleType, name),
            "{name} is missing"
        );
    }
    // Two full descriptions of at most 32 types each, after the names.
    let full = (requests.iter())
        .filter(|request| request["detail"] == "full")
        .count();
    assert_eq!(full, ModuleRegistry::default().types().count().div_ceil(32));
}

#[test]
fn a_module_type_card_holds_a_config_fragment_and_its_ports() {
    let (index, _) = gather(snapshot(false, Vec::new()));
    let filter = card(&index, FindKind::ModuleType, "filter");
    assert_eq!(filter.how["module"]["type"], "filter");
    assert!(filter.how["module"]["config"]["cutoff"].is_number());
    assert!(filter.how["inputs"]
        .as_array()
        .is_some_and(|ports| !ports.is_empty()));
    assert_eq!(filter.how["detail"]["tool"], "describe_module");
    assert!(filter.when.contains("cutoff"));
}

#[test]
fn a_loaded_piece_lists_each_control_with_the_call_that_reaches_it() {
    let registry = ModuleRegistry::default();
    let piece = snapshot(
        true,
        vec![
            module(&registry, "lead_filter", "filter", Some("Lead filter")),
            module(&registry, "tempo", "clock", None),
        ],
    );
    let (index, _) = gather(piece);
    assert!(index.piece_loaded());
    let cutoff = card(&index, FindKind::Control, "lead_filter.cutoff");
    assert_eq!(cutoff.how["call"]["tool"], "set_control");
    assert_eq!(cutoff.how["call"]["arguments"]["module_id"], "lead_filter");
    assert_eq!(cutoff.how["call"]["arguments"]["key"], "cutoff");
    assert!(cutoff.how["call"]["arguments"]["value"].is_number());
    assert!(cutoff.what.contains("Lead filter"));
    assert!(cutoff.when.contains(". Now "), "{}", cutoff.when);

    let answer = index.find(&FindQuery::words("make it brighter")).unwrap();
    assert_eq!(answer.cards[0].name, "lead_filter.cutoff");
    let answer = index.find(&FindQuery::words("faster tempo")).unwrap();
    assert_eq!(answer.cards[0].name, "tempo.bpm");

    let read_only = (index.entries().iter())
        .map(|entry| &entry.card)
        .find(|card| card.kind == FindKind::Control && card.how["call"]["tool"] == "get_control");
    assert!(read_only.is_some(), "a clock's position is read, not set");
}

#[test]
fn a_value_too_long_to_show_is_read_never_overwritten() {
    let registry = ModuleRegistry::default();
    let mut voice = module(&registry, "voice", "filter", None);
    let mut prompt = voice.controls[0].clone();
    prompt.meta.key = "prompt".into();
    prompt.meta.kind = crate::ControlKind::String { options: None };
    prompt.meta.read_only = false;
    prompt.meta.event = false;
    prompt.value = Some(crate::ControlValue::String("a long prompt ".repeat(20)));
    voice.controls.push(prompt);
    let entries = control_entries(&snapshot(true, vec![voice]));
    let card = &(entries.iter())
        .find(|entry| entry.card.name == "voice.prompt")
        .unwrap()
        .card;
    assert_eq!(card.how["call"]["tool"], "get_control");
    assert!(card.how["call"]["arguments"].get("value").is_none());
    assert!(card.when.contains("too long to show"), "{}", card.when);
}

#[test]
fn idle_daemon_reads_no_controls_and_the_answer_says_why() {
    let (index, _) = gather(snapshot(false, Vec::new()));
    assert!(!index.piece_loaded());
    assert!(!(index.entries().iter()).any(|entry| entry.card.kind == FindKind::Control));
    let query = FindQuery {
        kind: Some(FindKind::Control),
        ..FindQuery::words("cutoff")
    };
    let answer = index.find(&query).unwrap();
    assert!(answer.cards.is_empty());
    assert!(answer
        .notes
        .iter()
        .any(|note| note.starts_with("No piece is loaded")));
}

#[test]
fn every_page_of_a_catalog_is_read_and_a_failed_read_is_noted() {
    let (index, requests) = gather(snapshot(false, Vec::new()));
    let example = card(&index, FindKind::Example, "fugue.demo.bwv772");
    assert_eq!(example.how["call"]["tool"], "load_invention");
    assert_eq!(
        example.how["call"]["arguments"]["ref"]["package"],
        "fugue.demo.bwv772"
    );
    assert!(index.contains(FindKind::Example, "fugue.demo.drone"));
    assert!((requests.iter()).any(|request| request["query"]["cursor"] == "second"));
    let answer = index.find(&FindQuery::words("drone")).unwrap();
    assert!(answer
        .notes
        .contains(&"Developments could not be read: disk unreadable".to_string()));
}

#[test]
fn a_refused_type_batch_is_asked_in_halves_and_a_refused_type_keeps_its_name() {
    let (index, requests) = gather_refusing(snapshot(false, Vec::new()), 5, "reverb");
    for name in ModuleRegistry::default().types() {
        assert!(index.contains(FindKind::ModuleType, name), "{name}");
    }
    let reverb = card(&index, FindKind::ModuleType, "reverb");
    assert_eq!(reverb.when, "Could not be described: too large");
    assert_eq!(reverb.how["detail"]["tool"], "describe_module");
    let delay = card(&index, FindKind::ModuleType, "delay");
    assert!(delay.when.contains("Controls:"), "{}", delay.when);
    // The catalogs after the types are still read.
    assert!(index.contains(FindKind::Example, "fugue.demo.drone"));
    assert!(requests.len() < 200, "{} requests", requests.len());
}

#[test]
fn a_page_that_left_entries_out_says_so() {
    let (index, _) = gather(snapshot(false, Vec::new()));
    let answer = index.find(&FindQuery::words("tape")).unwrap();
    let note = "Packages: 1 could not be read and were not searched; list_packages shows why.";
    assert!(
        answer.notes.iter().any(|seen| seen == note),
        "{:?}",
        answer.notes
    );
}

#[test]
fn packages_are_listed_whatever_their_kind() {
    let (index, _) = gather(snapshot(false, Vec::new()));
    let tape = card(&index, FindKind::Package, "fugue.fx.tape-delay");
    assert_eq!(tape.how["call"]["arguments"]["id"], "fugue.fx.tape-delay");
    assert_eq!(tape.how["ref"]["package"], "fugue.fx.tape-delay");
    assert!(tape.what.contains("development package"));
    assert!(index.contains(FindKind::Package, "builtin"));
}

#[test]
fn a_development_card_declares_it_under_a_type_name() {
    let mut index = Index::new();
    let page = page(
        vec![content(
            "fugue.fx.tape-delay",
            ContentKind::Development,
            "A warm tape echo.",
        )],
        None,
    );
    index.extend(content_entries(&page));
    let card = card(&index, FindKind::Development, "fugue.fx.tape-delay");
    assert_eq!(card.how["declare"]["name"], "tape_delay");
    assert_eq!(card.how["module"]["type"], "tape_delay");
    assert_eq!(card.how["detail"]["tool"], "describe_development");
}

#[test]
fn an_unreachable_daemon_leaves_notes_not_errors() {
    let mut gather = Gather::new();
    let mut asked = 0;
    while let Some(_request) = gather.next_request() {
        asked += 1;
        gather.answer(Err("daemon unreachable".into()));
    }
    let index = gather.finish();
    // The snapshot, the type names, examples, developments and packages.
    assert_eq!(asked, 5);
    assert!(index.entries().is_empty());
    let answer = index.find(&FindQuery::words("reverb")).unwrap();
    assert!(answer
        .notes
        .iter()
        .any(|note| note == "Module types could not be read: daemon unreachable"));
}
