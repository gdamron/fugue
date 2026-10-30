//! Every registered module type, published beside a dac on a live graph,
//! keeps the audio thread clean: no allocation, no free and no lock as it
//! installs and as it is wired up, as requests write each declared control
//! (a scheduler ramping the ones automation writes, other threads writing
//! and reading its surface meanwhile), and as its inputs are written:
//! first with views tapping its output and the dac's, then with no tap.
//!
//! Each block renders while another thread holds every lock control threads
//! take on the live graph, and a module's own control-side state sits
//! behind a [`ControlLock`](crate::audio_thread::ControlLock), which panics
//! if a block takes it. A new module type is covered once it is registered.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use super::migrated::{block_while_held, LockingFactory};
use super::*;
use crate::module_config::tests::registry::{base_config, eight_frame_wav};
use crate::{ControlKind, ControlMeta, ControlSurface, ControlValue};

/// Types this test cannot exercise: (type, why). Empty: a type that truly
/// cannot be published here names itself with its reason, and an entry is
/// a bug to fix rather than a place to rest.
const NOT_COVERED: &[(&str, &str)] = &[];

/// Types whose blocks may allocate and free, though never lock: (type,
/// why). Empty: each entry would be a known bug, and must still allocate,
/// so a fix removes it.
const ALLOCATES: &[(&str, &str)] = &[];

pub(super) const DAC_ONLY: &str = r#"{
    "version": "1.0.0",
    "modules": [{ "id": "dac", "type": "dac", "config": { "soft_clip": false } }],
    "connections": []
}"#;

const SUBJECT: &str = "subject";

/// Blocks rendered while other threads write and read the surface.
const CONTENDED_BLOCKS: usize = 8;

/// The rig under test, and whether its subject's blocks are allowed to
/// allocate (see [`ALLOCATES`]) and have.
struct Subject {
    rig: Rig,
    may_allocate: bool,
    allocated: bool,
}

impl Subject {
    /// One block rendered with every control-side lock held elsewhere,
    /// which must neither wait for a lock nor allocate nor free.
    fn clean_block(&mut self, step: &str) -> Result<(), String> {
        match block_while_held(&mut self.rig, || ()) {
            Ok((0, 0)) => Ok(()),
            Ok(_) if self.may_allocate => {
                self.allocated = true;
                Ok(())
            }
            Ok((allocs, frees)) => Err(format!("{step}: {allocs} allocations, {frees} frees")),
            Err(waited) => Err(format!("{step}: {waited}")),
        }
    }

    fn clean_blocks(&mut self, step: &str, blocks: usize) -> Result<(), String> {
        (0..blocks).try_for_each(|block| self.clean_block(&format!("{step}, block {block}")))
    }

    /// Installs what is queued, then renders two clean blocks.
    fn clean_install(&mut self, step: &str) -> Result<(), String> {
        let ((), allocs, frees) = allocator_events(|| self.rig.graph.ensure_process_order());
        if (allocs, frees) != (0, 0) {
            return Err(format!(
                "{step}: install made {allocs} allocations, {frees} frees"
            ));
        }
        self.clean_blocks(step, 2)
    }
}

/// A value for `meta` other than `current` where one is easy to name, and
/// otherwise `current` itself, written back.
fn another(meta: &ControlMeta, current: &ControlValue) -> ControlValue {
    match (&meta.kind, current) {
        (ControlKind::Number { min, max, .. }, _) => {
            let span = max - min;
            let value = if span.is_finite() && span <= 1.0e6 {
                min + span / 2.0
            } else {
                min + 1.0
            };
            ControlValue::Number(value.round())
        }
        (ControlKind::Bool, ControlValue::Bool(value)) => ControlValue::Bool(!value),
        (
            ControlKind::String {
                options: Some(options),
            },
            ControlValue::String(now),
        ) => options.iter().find(|option| *option != now).map_or_else(
            || current.clone(),
            |option| ControlValue::String(option.clone()),
        ),
        _ => current.clone(),
    }
}

/// The surface's declared controls, each with the value to write it.
fn declared_writes(surface: &dyn ControlSurface) -> Vec<(String, ControlValue)> {
    let metas = surface.controls();
    let declared = metas.iter().filter(|meta| surface.declares(&meta.key));
    declared
        .filter_map(|meta| {
            let current = surface.get_control(&meta.key).ok()?;
            Some((meta.key.clone(), another(meta, &current)))
        })
        .collect()
}

/// A scheduler writing every control of `surface` automation can write on
/// its first clock edge, numbers ramped unless they are events (which fire
/// once); `None` when there are none.
fn ramping_schedule(surface: &dyn ControlSurface) -> Option<serde_json::Value> {
    let entry = |key: String, value: ControlValue| {
        let event = surface.automation(&key)?.event;
        let mut entry = serde_json::json!({ "at_step": 0, "module": SUBJECT, "control": key,
            "value": serde_json::to_value(&value).unwrap() });
        if matches!(value, ControlValue::Number(_)) && !event {
            entry["ramp_steps"] = serde_json::json!(2);
        }
        Some(entry)
    };
    let entries: Vec<_> = declared_writes(surface)
        .into_iter()
        .filter_map(|(key, value)| entry(key, value))
        .collect();
    (!entries.is_empty()).then(|| serde_json::json!({ "schedule": entries }))
}

/// Renders `blocks` clean blocks while two threads write every declared
/// control and read every control of `surface`, as front doors would.
fn contended(
    subject: &mut Subject,
    surface: &ControlSurfaceInstance,
    step: &str,
) -> Result<(), String> {
    /// Stops the workers however rendering ends, a panic included, so the
    /// scope can join them and the panic reach the caller.
    struct StopOnDrop<'a>(&'a AtomicBool);
    impl Drop for StopOnDrop<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }
    let stop = AtomicBool::new(false);
    let writes = declared_writes(surface.as_ref());
    thread::scope(|scope| {
        let _stop = StopOnDrop(&stop);
        for _ in 0..2 {
            scope.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    for (key, value) in &writes {
                        let _ = surface.set_control(key, value.clone());
                    }
                    for meta in surface.controls() {
                        let _ = surface.get_control(&meta.key);
                    }
                }
            });
        }
        subject.clean_blocks(step, CONTENDED_BLOCKS)
    })
}

/// What one type's exercise reached: declared writes accepted as
/// requests, controls a scheduler wrote, and whether a block allocated
/// (only ever true of a type in [`ALLOCATES`]).
#[derive(Default)]
struct Reached {
    writes: usize,
    scheduled: usize,
    allocated: bool,
}

/// Publishes `type_id` built from `config` beside a dac and puts it
/// through every step, stopping at the first that is not clean.
fn exercise(type_id: &str, config: &serde_json::Value) -> Result<Reached, String> {
    exercise_in(Rig::new(DAC_ONLY), type_id, config)
}

fn exercise_in(mut rig: Rig, type_id: &str, config: &serde_json::Value) -> Result<Reached, String> {
    let mut reached = Reached::default();
    rig.render(1);
    let may_allocate = ALLOCATES.iter().any(|(id, _)| *id == type_id);
    let built = GraphChange::build(&rig.registry, SAMPLE_RATE, SUBJECT, type_id, config)
        .map_err(|error| format!("did not build: {error}"))?;
    rig.live
        .edit(|change| {
            change.upsert(SUBJECT, built);
            Ok(())
        })
        .map_err(|error| format!("did not publish: {error}"))?;
    let mut subject = Subject {
        rig,
        may_allocate,
        allocated: false,
    };
    subject.clean_install("install")?;
    // Views tap what it plays until its inputs are written; the subject may
    // have no output to tap.
    let taps = [SUBJECT, "dac"].map(|id| subject.rig.live.tap_output(id, &[]).ok());
    if taps[1].is_none() {
        return Err("the dac could not be tapped".to_string());
    }

    let module = subject.rig.graph.modules[SUBJECT].module();
    let outputs: Vec<String> = module
        .outputs()
        .iter()
        .map(|port| port.to_string())
        .collect();
    let inputs: Vec<String> = module
        .inputs()
        .iter()
        .map(|port| port.to_string())
        .collect();
    if let Some(output) = outputs.first() {
        subject
            .rig
            .live
            .connect(edge(SUBJECT, output, "dac", "audio"))
            .map_err(|error| format!("did not connect: {error}"))?;
        subject.clean_install("wiring to the dac")?;
    }

    let surface = subject
        .rig
        .live
        .control_surfaces
        .lock()
        .unwrap()
        .get(SUBJECT)
        .cloned();
    if let Some(surface) = &surface {
        let writes = declared_writes(surface.as_ref());
        for (key, value) in &writes {
            // A refusal (a read-only control, say) is the control thread's.
            reached.writes += usize::from(surface.set_control(key, value.clone()).is_ok());
        }
        if reached.writes == 0 && !writes.is_empty() {
            return Err(format!("refused every declared write: {writes:?}"));
        }
        subject.clean_blocks("writes", 2)?;

        if let Some(schedule) = ramping_schedule(surface.as_ref()) {
            reached.scheduled = schedule["schedule"].as_array().map_or(0, Vec::len);
            let scheduler = subject.rig.build("sched", "control_scheduler", schedule);
            subject
                .rig
                .live
                .edit(|change| {
                    change.upsert("sched", scheduler);
                    Ok(())
                })
                .map_err(|error| format!("its schedule was refused: {error}"))?;
            subject.clean_install("adding a scheduler")?;
            for gate in [1.0, 0.0, 1.0, 0.0, 1.0] {
                subject
                    .rig
                    .live
                    .write_input("sched", "clock", gate)
                    .unwrap();
                subject.clean_block(&format!("ramping, clock {gate}"))?;
            }
        }
        contended(&mut subject, surface, "contended writes")?;
    }

    drop(taps);
    for port in &inputs {
        subject.rig.live.write_input(SUBJECT, port, 0.5).unwrap();
    }
    subject.clean_blocks("input writes", 2)?;
    reached.allocated = subject.allocated;
    Ok(reached)
}

/// The configs each registered type is exercised with.
fn configs(registry: &ModuleRegistry) -> Vec<(String, serde_json::Value)> {
    let mut configs = Vec::new();
    for type_id in registry.types() {
        if NOT_COVERED.iter().any(|(id, _)| *id == type_id) {
            continue;
        }
        let config = match type_id {
            "wasm_module" => wasm_config(),
            // A live sink writes its file: keep it out of the source tree.
            "audio_file_sink" => serde_json::json!({ "path": scratch().join("sink.wav") }),
            // A stand-in ffmpeg and a local destination: no real encoder,
            // no network.
            "rtmp_sink" | "youtube_sink" => {
                let mut config = base_config(type_id);
                config["ffmpeg_path"] = serde_json::json!(fake_ffmpeg());
                if type_id == "youtube_sink" {
                    config["server_url"] = serde_json::json!("rtmps://127.0.0.1:9/live2");
                } else {
                    config["url"] = serde_json::json!("rtmp://127.0.0.1:9/live");
                }
                config
            }
            // Its controls are its samples' slots.
            "sample_kit" => serde_json::json!({
                "samples": [{ "key": 36, "asset": eight_frame_wav() }]
            }),
            _ => base_config(type_id),
        };
        if type_id == "sample_slicer" {
            // Its controls exist only in elastic mode.
            let mut elastic = config.clone();
            elastic["mode"] = serde_json::json!("elastic");
            configs.push((type_id.to_string(), elastic));
        }
        configs.push((type_id.to_string(), config));
    }
    configs
}

/// A directory for files the exercised modules write, kept for the test
/// process.
fn scratch() -> &'static std::path::Path {
    static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    DIR.get_or_init(|| tempfile::tempdir().unwrap()).path()
}

/// An ffmpeg that answers the probe and exits at once, so a streaming
/// sink's workers start and wind down without an encoder or a network.
fn fake_ffmpeg() -> &'static std::path::Path {
    static PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        #[cfg(windows)]
        let (name, script) = ("ffmpeg.cmd", "@exit /b 0\r\n");
        #[cfg(not(windows))]
        let (name, script) = ("ffmpeg", "#!/bin/sh\nexit 0\n");
        let path = scratch().join(name);
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    })
}

#[cfg(all(feature = "plugins", not(target_arch = "wasm32")))]
fn wasm_config() -> serde_json::Value {
    use std::sync::OnceLock;
    // The fixture guest, built once and kept for the test process.
    static MANIFEST: OnceLock<(tempfile::TempDir, std::path::PathBuf)> = OnceLock::new();
    let (_, manifest) = MANIFEST.get_or_init(|| {
        let (dir, _) = crate::plugins::wasm::alloc_tests::fixture_component();
        let manifest = dir.path().join("fugue-module.json");
        let text = serde_json::json!({
            "id": "fugue.test.fixture", "version": "1.0.0", "kind": "module",
            "license": "MIT", "authors": [{ "name": "Fugue Test" }],
            "targets": ["in-graph-agent"], "entry": { "wasm": "fixture.fugue-module.wasm" }
        });
        std::fs::write(&manifest, text.to_string()).unwrap();
        (dir, manifest)
    });
    serde_json::json!({ "manifest": manifest })
}

#[cfg(not(all(feature = "plugins", not(target_arch = "wasm32"))))]
fn wasm_config() -> serde_json::Value {
    unreachable!("wasm_module is registered only with plugins")
}

#[test]
fn no_module_type_allocates_frees_or_locks_on_the_audio_thread() {
    #[cfg(all(feature = "plugins", not(target_arch = "wasm32")))]
    let _cache = crate::plugins::wasm::alloc_tests::temp_cache();
    let registry = ModuleRegistry::default();
    let configs = configs(&registry);
    let mut failures = Vec::new();
    let (mut written, mut scheduled, mut allocated) = (Vec::new(), Vec::new(), Vec::new());
    for (type_id, config) in &configs {
        let outcome = catch_unwind(AssertUnwindSafe(|| exercise(type_id, config)));
        match outcome {
            Ok(Ok(reached)) => {
                if reached.writes > 0 {
                    written.push(type_id.as_str());
                }
                if reached.scheduled > 0 {
                    scheduled.push(type_id.as_str());
                }
                if reached.allocated {
                    allocated.push(type_id.as_str());
                }
            }
            Ok(Err(why)) => failures.push(format!("'{type_id}' {config}: {why}")),
            Err(panic) => {
                let why = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|why| why.to_string()))
                    .unwrap_or_default();
                failures.push(format!("'{type_id}' {config}: panicked: {why}"));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    // The configs reach each kind of control: payloads, slots, events.
    for expected in [
        "control_scheduler",
        "sample_kit",
        "sample_instrument",
        "step_sequencer",
    ] {
        assert!(
            written.contains(&expected),
            "'{expected}' not written: {written:?}"
        );
    }
    assert!(scheduled.len() >= 15, "{scheduled:?}");
    let covered: Vec<&str> = configs.iter().map(|(id, _)| id.as_str()).collect();
    for expected in [
        "oscillator",
        "dac",
        "control_scheduler",
        "sample_instrument",
        "agent",
    ] {
        assert!(
            covered.contains(&expected),
            "'{expected}' not covered: {covered:?}"
        );
    }
    #[cfg(all(feature = "plugins", not(target_arch = "wasm32")))]
    assert!(covered.contains(&"wasm_module"), "{covered:?}");
    for (type_id, why) in NOT_COVERED {
        assert!(
            registry.has_type(type_id),
            "'{type_id}' ({why}) is not registered"
        );
    }
    // An allowance outlives its bug only as long as the bug does.
    for (type_id, why) in ALLOCATES {
        assert!(
            !registry.has_type(type_id) || allocated.contains(type_id),
            "'{type_id}' no longer allocates ({why}): remove its entry"
        );
    }
}

/// Exercises a module running `each_block` in its `process()`.
fn exercise_running(each_block: impl Fn() + Send + Sync + 'static) -> String {
    let mut rig = Rig::new(DAC_ONLY);
    rig.registry.register(LockingFactory(Arc::new(each_block)));
    rig.adopt_registry();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        exercise_in(rig, "locking", &serde_json::json!({}))
    }));
    match outcome {
        Ok(result) => result.err().expect("a dirty block passed"),
        Err(panic) => panic.downcast_ref::<String>().cloned().unwrap_or_default(),
    }
}

/// The negative controls: a block that allocates, or takes a lock only
/// control threads may take, is caught.
#[test]
fn a_module_that_allocates_or_takes_a_control_lock_is_caught() {
    let caught = exercise_running(|| drop(std::hint::black_box(vec![0u8; 8])));
    assert_eq!(caught, "install, block 0: 1 allocations, 1 frees");

    // Release builds compile the control lock's check out.
    if cfg!(debug_assertions) {
        let lock = Arc::new(crate::audio_thread::ControlLock::new(()));
        let caught = exercise_running(move || drop(lock.lock()));
        assert!(caught.contains("taking a control lock"), "{caught}");
    }
}
