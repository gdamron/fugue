//! The guest world this host loads (`wit/fugue-module.wit`), and the check
//! that refuses a guest built for another version of it before it runs.

use wasmtime::component::{Component, ComponentExportIndex};
use wasmtime::Engine;

/// The version of the `fugue:module` world this host loads. Only guests
/// built for exactly this version load, so every bump is a breaking one.
pub(super) const WORLD_VERSION: &str = "0.2.0";

/// The world's host interface, as a guest imports it.
pub(super) const HOST_INTERFACE: &str = "fugue:module/host@0.2.0";

/// The world's guest interface, as every guest exports it.
pub(super) const GUEST_INTERFACE: &str = "fugue:module/guest@0.2.0";

/// Exports only the first world, 0.1.0, had: its guests exported their
/// functions bare and returned each output block as a list, which the host
/// had to allocate to read.
const FIRST_WORLD_EXPORTS: &[&str] = &["output-block", "process-output-block"];
const FIRST_WORLD_VERSION: &str = "0.1.0";

/// Finds the guest's `guest` interface, refusing a guest built for another
/// version of the world with a message naming the version it was built for
/// and the one it needs. It reads only the component's type, so no guest
/// code has run.
pub(super) fn check_world(
    engine: &Engine,
    component: &Component,
) -> Result<ComponentExportIndex, String> {
    let ty = component.component_type();
    let mut first_world = false;
    for (name, _) in ty.exports(engine) {
        first_world |= FIRST_WORLD_EXPORTS.contains(&name);
        let Some(rest) = name.strip_prefix("fugue:module/guest") else {
            continue;
        };
        let version = match rest.strip_prefix('@') {
            Some(version) => version,
            None if rest.is_empty() => "(unversioned)",
            // Another interface whose name starts the same, such as `guest-extra`.
            None => continue,
        };
        // Exact: wasmtime's lookup would also take a semver-compatible one.
        return if version == WORLD_VERSION {
            component
                .get_export_index(None, GUEST_INTERFACE)
                .ok_or_else(|| refusal(WORLD_VERSION))
        } else {
            Err(refusal(version))
        };
    }
    Err(if first_world {
        refusal(FIRST_WORLD_VERSION)
    } else {
        format!(
            "this wasm module exports no fugue:module/guest interface, so it is not a \
             Fugue module: build it against fugue:module@{WORLD_VERSION} \
             (wit/fugue-module.wit)"
        )
    })
}

fn refusal(built_for: &str) -> String {
    format!(
        "this wasm module was built for the fugue:module@{built_for} world, and this \
         host loads fugue:module@{WORLD_VERSION} modules: rebuild it against \
         fugue:module@{WORLD_VERSION} (wit/fugue-module.wit), which writes each \
         output block with the host's `emit-output` import"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_a_first_world_guest_is_refused_before_it_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("old.fugue-module.wasm");
        std::fs::write(&path, FIRST_WORLD_GUEST).expect("write guest");
        let manifest = super::super::alloc_tests::fixture_manifest();
        let error = match super::super::load_component_module(&path, 48_000, "{}", &manifest) {
            Ok(_) => panic!("a first-world guest loaded"),
            Err(error) => error.to_string(),
        };
        assert!(
            error.contains("rebuild it against fugue:module@0.2.0"),
            "{error}"
        );
    }

    fn check(wat: &str) -> Result<(), String> {
        let engine = super::super::component_engine().expect("engine");
        let component = Component::new(&engine, wat).expect("component");
        check_world(&engine, &component).map(|_| ())
    }

    /// A component exporting an empty instance as `name`.
    fn exporting(name: &str) -> String {
        format!(r#"(component (instance $guest) (export "{name}" (instance $guest)))"#)
    }

    /// A stand-in for a first-world guest: its block exports, by name.
    const FIRST_WORLD_GUEST: &str = r#"(component
        (core module $m (func (export "f") (param i32) (result i32) i32.const 0))
        (core instance $i (instantiate $m))
        (func (export "output-block") (param "index" u32) (result u32)
            (canon lift (core func $i "f"))))"#;

    #[test]
    fn a_first_world_guest_is_refused_naming_the_version_it_needs() {
        let refusal = check(FIRST_WORLD_GUEST).unwrap_err();
        assert!(
            refusal.contains("built for the fugue:module@0.1.0 world")
                && refusal.contains("rebuild it against fugue:module@0.2.0"),
            "{refusal}"
        );
    }

    #[test]
    fn a_guest_exporting_another_version_is_refused() {
        let refusal = check(&exporting("fugue:module/guest@0.2.1")).unwrap_err();
        assert!(
            refusal.contains("built for the fugue:module@0.2.1 world")
                && refusal.contains("rebuild it against fugue:module@0.2.0"),
            "{refusal}"
        );
    }

    #[test]
    fn a_component_that_is_no_guest_is_refused() {
        for wat in [
            "(component)".to_string(),
            exporting("fugue:module/guest-extra@0.2.0"),
        ] {
            let refusal = check(&wat).unwrap_err();
            assert!(
                refusal.contains("exports no fugue:module/guest"),
                "{refusal}"
            );
        }
    }

    #[test]
    fn the_world_version_is_the_wit_package_version() {
        let wit = include_str!("../../../wit/fugue-module.wit");
        assert!(wit.starts_with(&format!("package fugue:module@{WORLD_VERSION};")));
        assert_eq!(HOST_INTERFACE, format!("fugue:module/host@{WORLD_VERSION}"));
        assert_eq!(
            GUEST_INTERFACE,
            format!("fugue:module/guest@{WORLD_VERSION}")
        );
    }

    /// One with no outputs, too: it calls nothing on the host, so it has no
    /// host import, but its export still names its version.
    #[test]
    fn a_guest_exporting_this_version_passes() {
        assert_eq!(check(&exporting(GUEST_INTERFACE)), Ok(()));
    }
}
