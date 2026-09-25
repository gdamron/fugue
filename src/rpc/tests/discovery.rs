use super::*;
use crate::ModuleRegistry;

#[test]
fn full_discovery_exposes_metadata_or_an_explicit_error() {
    let registry = ModuleRegistry::default();
    let catalog = ModuleTypeList::from_registry(
        &registry,
        44_100,
        RegistryScope::Builtins,
        &ModuleTypeQuery {
            types: Some(vec!["oscillator".into(), "audio_file_sink".into()]),
            detail: TypeDetail::Full,
        },
    )
    .unwrap();
    let entries = catalog.details.unwrap();
    assert!(
        matches!(&entries[0], ModuleTypeDetail::Unavailable { type_name, error } if type_name == "audio_file_sink" && error.code == RpcErrorCode::ModuleBuildFailed)
    );
    match &entries[1] {
        ModuleTypeDetail::Available { info } => {
            assert_eq!(info.type_name, "oscillator");
            assert!(info.outputs.contains(&"audio".into()));
            assert!(info.controls.iter().any(|c| c.key == "frequency"));
        }
        _ => panic!("oscillator defaults should be inspectable"),
    }
    let json = serde_json::to_value(&entries[0]).unwrap();
    assert!(json.get("controls").is_none());
    assert!(json.get("inputs").is_none());
}
