//! A wasm guest that fails mid-block is signalled for a control thread to
//! report: the block neither waits for a lock nor prints.
//!
//! The failing block does allocate, inside wasmtime, which builds the trap
//! it returns; what the module adds is checked in `plugins::wasm`.

use super::catalog::DAC_ONLY;
use super::migrated::block_while_held;
use super::*;
use crate::plugins::wasm::alloc_tests::{fixture_component, temp_cache};
use crate::plugins::wasm::failure_tests::failing_guest;
use crate::{GraphModule, ModuleBuildResult, ModuleFactory, WasmModuleHandle};

/// Builds fixture guests that trap on their first block, keeping each
/// one's handle.
struct FailingGuest {
    path: std::path::PathBuf,
    handles: Arc<Mutex<Vec<WasmModuleHandle>>>,
}

impl ModuleFactory for FailingGuest {
    fn type_id(&self) -> &'static str {
        "failing_guest"
    }

    fn build(
        &self,
        _sample_rate: u32,
        _config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let (module, handle): (GraphModule, _) = failing_guest(&self.path);
        self.handles.lock().unwrap().push(handle);
        Ok(ModuleBuildResult {
            module,
            handles: Vec::new(),
            control_surface: None,
            sink: None,
        })
    }
}

#[test]
fn a_guest_that_fails_mid_block_is_signalled_without_waiting_for_a_lock() {
    let _cache = temp_cache();
    let (_dir, path) = fixture_component();
    let handles = Arc::new(Mutex::new(Vec::new()));
    let mut rig = Rig::new(DAC_ONLY);
    rig.registry.register(FailingGuest {
        path,
        handles: Arc::clone(&handles),
    });
    rig.adopt_registry();
    let built = rig.build("guest", "failing_guest", serde_json::json!({}));
    rig.live
        .edit(|change| {
            change.upsert("guest", built);
            Ok(())
        })
        .expect("publish");
    rig.live
        .connect(edge("guest", "audio", "dac", "audio"))
        .expect("wire");
    rig.graph.ensure_process_order();

    // Every lock control threads take is held elsewhere while it fails.
    block_while_held(&mut rig, || ()).expect("the failing block waited for a lock");

    let handles = handles.lock().unwrap();
    let reported: Vec<String> = handles.iter().filter_map(|h| h.failure()).collect();
    assert_eq!(reported.len(), 1, "one guest ran and failed: {reported:?}");
    assert!(
        reported[0].contains("failed in `process`") && reported[0].ends_with("; muting output"),
        "{}",
        reported[0]
    );
}
