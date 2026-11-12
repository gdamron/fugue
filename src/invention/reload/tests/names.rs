//! A document's module names on load and reload: presentation only, so a
//! difference in them alone rebuilds nothing.

use super::*;

/// BASE with `osc1` named `name`.
fn named(name: Option<&str>) -> Invention {
    let mut document = doc(BASE);
    let spec = (document.modules.iter_mut())
        .find(|spec| spec.id == "osc1")
        .unwrap();
    spec.name = name.map(str::to_string);
    document
}

fn name_of(document: &Invention) -> Option<String> {
    let spec = document.modules.iter().find(|spec| spec.id == "osc1");
    spec.and_then(|spec| spec.name.clone())
}

#[test]
fn a_name_only_reload_rebuilds_nothing_and_keeps_the_new_name() {
    let mut running = start(BASE);
    let before = running.pump.render(4);
    let generation = running.live.generation();

    let report = running.reload(named(Some("Lead"))).unwrap();

    assert!(report.added.is_empty() && report.removed.is_empty() && report.swapped.is_empty());
    assert!(report.controls_updated.is_empty());
    assert_eq!(report.unchanged, 3);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(running.live.generation(), generation);
    let (fresh, _) = InventionBuilder::new(48_000).build(doc(BASE)).unwrap();
    let fresh = Settled::start(fresh);
    let mut expected = fresh.pump.render(8);
    assert_eq!(&expected[..before.len()], &before[..]);
    let rest = expected.split_off(before.len());
    assert_eq!(running.pump.render(4), rest);
    assert_eq!(
        name_of(&running.document().unwrap()).as_deref(),
        Some("Lead")
    );

    let report = running.reload(doc(BASE)).unwrap();
    assert!(report.swapped.is_empty());
    assert_eq!(name_of(&running.document().unwrap()), None);
}

#[test]
fn a_load_drops_a_name_that_cannot_be_used_with_a_warning() {
    let (runtime, _) = InventionBuilder::new(48_000)
        .build(named(Some("two\nlines")))
        .unwrap();
    let state = runtime.state.lock().unwrap();
    let warnings: Vec<String> = (state.load_warnings.iter())
        .map(ToString::to_string)
        .collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("'osc1'"), "{warnings:?}");
    assert_eq!(name_of(state.document.as_ref().unwrap()), None);
}

#[test]
fn a_running_module_is_described_with_its_name_and_its_id() {
    let running = start(&named(Some("Lead")).to_json().unwrap());
    let query = crate::DescribeModuleQuery {
        module_id: Some("osc1".into()),
        ..Default::default()
    };
    let description = running.describe_module(&query).unwrap();
    assert_eq!(description.module_id.as_deref(), Some("osc1"));
    assert_eq!(description.name.as_deref(), Some("Lead"));
    let unnamed = crate::DescribeModuleQuery {
        module_id: Some("osc2".into()),
        ..Default::default()
    };
    let description = running.describe_module(&unnamed).unwrap();
    assert_eq!(description.name, None);
    let wire = serde_json::to_value(&description).unwrap();
    assert!(wire.get("name").is_none(), "{wire}");
}
