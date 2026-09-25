use super::*;
use crate::ModuleRegistry;

#[test]
fn built_in_packages_list_registry_types() {
    let registry = ModuleRegistry::default();
    let packages = PackageList::built_in(&registry);
    assert_eq!(packages.packages.len(), 1);
    assert_eq!(packages.packages[0].source, PackageSource::BuiltIn);
    assert!(packages.packages[0]
        .module_types
        .contains(&"oscillator".to_string()));
}
