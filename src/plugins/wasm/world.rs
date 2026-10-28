//! The guest world this host loads (`wit/fugue-module.wit`), and the check
//! that refuses a guest built for another version of it before it runs.

use wasmtime::component::Component;
use wasmtime::Engine;

/// The version of the `fugue:module` world this host loads.
pub(super) const WORLD_VERSION: &str = "0.2.0";

/// The world's host interface, as a guest imports it.
pub(super) const HOST_INTERFACE: &str = "fugue:module/host@0.2.0";

/// Exports only the first world, 0.1.0, had: its guests returned each
/// output block as a list, which the host had to allocate to read.
const FIRST_WORLD_EXPORTS: &[&str] = &["output-block", "process-output-block"];
const FIRST_WORLD_VERSION: &str = "0.1.0";

/// Refuses a guest built for another version of the world, naming the
/// version it was built for and the one it needs. A guest that imports
/// nothing from the world (it has no outputs) and exports no first-world
/// function cannot be told apart, and loads.
pub(super) fn check_world(engine: &Engine, component: &Component) -> Result<(), String> {
    let ty = component.component_type();
    for (name, _) in ty.imports(engine) {
        let Some((_, version)) = name
            .strip_prefix("fugue:module/")
            .and_then(|rest| rest.split_once('@'))
        else {
            continue;
        };
        if version != WORLD_VERSION {
            return Err(refusal(version));
        }
    }
    if ty
        .exports(engine)
        .any(|(name, _)| FIRST_WORLD_EXPORTS.contains(&name))
    {
        return Err(refusal(FIRST_WORLD_VERSION));
    }
    Ok(())
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

    fn check(wat: &str) -> Result<(), String> {
        let engine = super::super::component_engine().expect("engine");
        let component = Component::new(&engine, wat).expect("component");
        check_world(&engine, &component)
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
    fn a_guest_importing_another_host_version_is_refused() {
        let refusal =
            check(r#"(component (import "fugue:module/host@0.3.0" (instance)))"#).unwrap_err();
        assert!(
            refusal.contains("built for the fugue:module@0.3.0 world")
                && refusal.contains("rebuild it against fugue:module@0.2.0"),
            "{refusal}"
        );
    }

    #[test]
    fn the_world_version_is_the_wit_package_version() {
        let wit = include_str!("../../../wit/fugue-module.wit");
        assert!(wit.starts_with(&format!("package fugue:module@{WORLD_VERSION};")));
        assert_eq!(HOST_INTERFACE, format!("fugue:module/host@{WORLD_VERSION}"));
    }

    #[test]
    fn a_guest_importing_this_host_version_passes() {
        let wat = format!(r#"(component (import "{HOST_INTERFACE}" (instance)))"#);
        assert_eq!(check(&wat), Ok(()));
    }
}
