//! A failing guest is signalled, not printed, on the audio thread.

use super::alloc_tests::{fixture_component, fixture_manifest, temp_cache};
use super::failure::Reason;
use super::*;
use crate::alloc_counter::allocator_events;
use crate::audio_thread::AudioThreadScope;

#[test]
fn a_trapping_guest_mutes_the_module_and_is_reported_through_its_handle() {
    let _cache = temp_cache();
    let (_temp, path) = fixture_component();
    let (mut module, handle) =
        load_component_parts(&path, 48_000, "{}", &fixture_manifest()).expect("load");
    assert_eq!(handle.failure(), None, "nothing has failed yet");
    assert!(module.process(64));

    module.run_out_of_fuel();
    let active = {
        let _audio = AudioThreadScope::enter();
        module.process(64)
    };

    assert!(!active);
    assert!(module.output_block(0)[..64]
        .iter()
        .all(|sample| *sample == 0.0));
    let message = handle.failure().expect("the trap was recorded");
    assert!(
        message.starts_with("wasm module 'FixtureOscillator' failed in `process`: "),
        "{message}"
    );
    assert!(message.ends_with("; muting output"), "{message}");
    // Latched: it stops calling in, and the first failure stands.
    assert!(!module.process(64));
    assert_eq!(handle.failure(), Some(message));
}

#[test]
fn a_guest_error_is_reported_with_the_guests_own_message() {
    let _cache = temp_cache();
    let (_temp, path) = fixture_component();
    let (mut module, handle) =
        load_component_parts(&path, 48_000, "{}", &fixture_manifest()).expect("load");

    // The fixture guest refuses an input index it does not have.
    module.input_connected = vec![true, true];
    module.input_blocks = vec![[0.0; MAX_BLOCK]; 2];
    assert!(!module.process(64));

    let message = handle.failure().expect("the refusal was recorded");
    assert_eq!(
        message,
        "wasm module 'FixtureOscillator' failed in `set-input-block`: \
         unknown input index: 1; muting output"
    );
}

#[test]
fn signalling_a_failure_makes_no_host_allocation_and_takes_no_lock() {
    let _cache = temp_cache();
    let (_temp, path) = fixture_component();
    let (mut module, handle) =
        load_component_parts(&path, 48_000, "{}", &fixture_manifest()).expect("load");
    // Built here: what the audio thread moves in already exists.
    let reason = Reason::Guest("refused".to_string());

    let (active, allocs, frees) = allocator_events(|| {
        let _audio = AudioThreadScope::enter();
        module.fail("process", reason)
    });

    assert!(!active);
    assert_eq!((allocs, frees), (0, 0));
    let message = handle.failure().expect("recorded");
    assert!(
        message.contains("failed in `process`: refused"),
        "{message}"
    );
}

/// A loaded fixture guest whose next call traps, with the handle that
/// reads its failure.
pub(crate) fn failing_guest(path: &std::path::Path) -> (GraphModule, WasmModuleHandle) {
    let (mut module, handle) =
        load_component_parts(path, 48_000, "{}", &fixture_manifest()).expect("load");
    module.run_out_of_fuel();
    (GraphModule::Module(Box::new(module)), handle)
}
