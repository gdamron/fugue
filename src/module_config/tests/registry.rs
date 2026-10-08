//! Every registered module type reads its declared config keys by the
//! reader's rules, and refuses a non-finite number for each of its number
//! controls (a module's config must accept each control key, see
//! `apply_control_keys`).

use super::probe::{LegacyProbeFactory, ProbeFactory};
use super::*;
use crate::{ControlKind, ControlValue, GraphModule, ModuleRegistry, DEFAULT_BLOCK_SIZE};

/// Types whose factories still read numbers their own way, exempt from the
/// control-key and completeness checks. Every built-in factory reads through
/// [`ConfigReader`], so it is empty: a new factory declares its keys in
/// `config_keys()` rather than joining it.
pub(super) const NOT_YET_MIGRATED: &[&str] = &[];

/// Number controls a factory does not read from config, so a config value
/// for them is ignored rather than refused: (type, key, why).
const CONTROLS_NOT_READ_FROM_CONFIG: &[(&str, &str, &str)] = &[
    ("agent", "request_count", "a counter the runtime keeps"),
    ("agent", "trigger_count", "a counter the gate input keeps"),
    ("agent", "reset_count", "a counter the reset input keeps"),
    ("cell_sequencer", "loop_count", "read-only telemetry"),
    ("cell_sequencer", "current_cell", "read-only telemetry"),
    ("cell_sequencer", "total_cells", "read-only telemetry"),
    (
        "cell_sequencer",
        "advance",
        "an action: a write advances the bank",
    ),
    (
        "sample_instrument",
        "note_on",
        "an action: a write starts a note",
    ),
    (
        "sample_instrument",
        "note_off",
        "an action: a write releases a note",
    ),
    ("control_scheduler", "step", "read-only playhead"),
];

/// Types the harness cannot build: they need what a test has not got.
pub(crate) const UNBUILDABLE: &[(&str, &str)] = &[("wasm_module", "needs a compiled guest module")];

/// The config each type is built from before a key is added: enough for
/// types whose config is mandatory (assets, say) to build.
pub(crate) fn base_config(type_id: &str) -> Value {
    let size = json!({ "width": 640, "height": 360 });
    match type_id {
        "audio_file_sink" => json!({ "path": "never-written.wav" }),
        "cell_sequencer" => json!({ "sequences": [[60, null], [62]] }),
        "rtmp_sink" => with(&size, "url", json!("rtmp://example.test/live")),
        "sample_slicer" => json!({
            "asset": { "path": eight_frame_wav() },
            "slices": (0..4)
                .map(|n| json!({ "start_frames": 2 * n, "end_frames": 2 * n + 2 }))
                .collect::<Vec<_>>(),
        }),
        "youtube_sink" => with(&size, "stream_key", json!("test-stream-key")),
        _ => json!({}),
    }
}

/// A silent eight-frame WAV, written once per test process.
pub(crate) fn eight_frame_wav() -> &'static str {
    static PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let name = format!("fugue-module-config-{}.wav", std::process::id());
        let path = std::env::temp_dir().join(name);
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        (0..8).for_each(|_| writer.write_sample(0_i16).unwrap());
        writer.finalize().unwrap();
        path.to_string_lossy().into_owned()
    })
}

/// `base` (an object) with `key` set to `value`.
fn with(base: &Value, key: &str, value: Value) -> Value {
    let mut config = base.clone();
    config[key] = value;
    config
}

/// What a built module shows: its ports, every control's value, and the
/// samples a few blocks of processing write to each output.
#[derive(Debug, PartialEq)]
struct Observed {
    inputs: Vec<String>,
    outputs: Vec<String>,
    controls: Vec<(String, Result<ControlValue, String>)>,
    samples: Vec<Vec<f32>>,
}

fn observe(registry: &ModuleRegistry, type_id: &str, config: &Value) -> Result<Observed, String> {
    let mut built = registry
        .for_validation()
        .build(type_id, 48_000, config)
        .map_err(|error| error.to_string())?;
    let controls = built
        .control_surface
        .map(|surface| {
            surface
                .controls()
                .into_iter()
                .map(|meta| {
                    let value = surface.get_control(&meta.key);
                    (meta.key, value)
                })
                .collect()
        })
        .unwrap_or_default();
    let module = built.module.module_mut();
    let inputs = module
        .inputs()
        .iter()
        .map(|port| port.to_string())
        .collect();
    let outputs: Vec<String> = module.outputs().iter().map(|p| p.to_string()).collect();
    let mut samples = vec![Vec::new(); outputs.len()];
    // A sink's inspection build is inert: it has ports but plays nothing.
    if let (GraphModule::Module(module), None) = (&mut built.module, built.sink) {
        for _ in 0..4 {
            module.process(DEFAULT_BLOCK_SIZE);
            for (index, samples) in samples.iter_mut().enumerate() {
                samples.extend_from_slice(&module.output_block(index)[..DEFAULT_BLOCK_SIZE]);
            }
        }
    }
    Ok(Observed {
        inputs,
        outputs,
        controls,
        samples,
    })
}

/// Checks each declared key of `type_id`, pushing what fails to `misses`.
fn check_declared_keys(registry: &ModuleRegistry, type_id: &str, misses: &mut Vec<String>) {
    let base = base_config(type_id);
    for key in registry.config_keys(type_id) {
        let name = format!("{type_id}.{}", key.key);
        match key.kind {
            ConfigKind::Integer { min, max } => {
                let n = 3.clamp(min, max);
                let as_integer = observe(registry, type_id, &with(&base, key.key, json!(n as i64)));
                let as_float = observe(registry, type_id, &with(&base, key.key, json!(n as f64)));
                match (&as_integer, &as_float) {
                    (Ok(integer), Ok(float)) if integer == float => {}
                    _ => misses.push(format!(
                        "{name}: {n}.0 builds as {as_float:?}, {n} as {as_integer:?}"
                    )),
                }
                let fraction = json!(n as f64 + 0.5);
                match observe(registry, type_id, &with(&base, key.key, fraction.clone())) {
                    Err(error)
                        if error.contains(&format!("'{}'", key.key))
                            && error.contains("expects a whole number") => {}
                    other => misses.push(format!("{name}: {fraction} gave {other:?}")),
                }
            }
            ConfigKind::Text | ConfigKind::Bool | ConfigKind::Json => {}
            ConfigKind::Float => {
                expect_not_finite(registry, type_id, &base, key.key, misses);
                if let Err(error) =
                    observe(registry, type_id, &with(&base, key.key, json!(f32::MAX)))
                {
                    misses.push(format!("{name}: f32::MAX refused: {error}"));
                }
            }
        }
    }
}

/// Pushes a miss unless `{key: 1e39}` is refused as not finite.
fn expect_not_finite(
    registry: &ModuleRegistry,
    type_id: &str,
    base: &Value,
    key: &str,
    misses: &mut Vec<String>,
) {
    expect_1e39_refused(registry, type_id, base, key, "a finite number", misses);
}

/// Pushes a miss unless `{key: 1e39}` is refused as not `expected`.
fn expect_1e39_refused(
    registry: &ModuleRegistry,
    type_id: &str,
    base: &Value,
    key: &str,
    expected: &str,
    misses: &mut Vec<String>,
) {
    match observe(registry, type_id, &with(base, key, json!(1e39))) {
        Err(error)
            if error.contains(&format!("'{key}' expects {expected}"))
                && error.ends_with("got 1e39") => {}
        other => misses.push(format!("{type_id}.{key}: 1e39 gave {other:?}")),
    }
}

/// Checks that each number control of `type_id` refuses `1e39` in config:
/// as not finite, or as not whole for a control declared an integer key.
fn check_control_keys(registry: &ModuleRegistry, type_id: &str, misses: &mut Vec<String>) {
    let base = base_config(type_id);
    let built = match registry.for_validation().build(type_id, 48_000, &base) {
        Ok(built) => built,
        Err(error) => return misses.push(format!("{type_id}: base config refused: {error}")),
    };
    let Some(surface) = built.control_surface else {
        return;
    };
    let not_read = |key: &str| {
        CONTROLS_NOT_READ_FROM_CONFIG
            .iter()
            .any(|(id, not_read, _)| *id == type_id && *not_read == key)
    };
    for meta in surface.controls() {
        if !matches!(meta.kind, ControlKind::Number { .. }) || not_read(&meta.key) {
            continue;
        }
        let integer = registry
            .config_keys(type_id)
            .iter()
            .any(|key| key.key == meta.key && matches!(key.kind, ConfigKind::Integer { .. }));
        let expected = if integer {
            "a whole number"
        } else {
            "a finite number"
        };
        expect_1e39_refused(registry, type_id, &base, &meta.key, expected, misses);
    }
}

/// Runs every check over `registry`, sparing `exempt` the control-key and
/// completeness checks.
fn check_registry(registry: &ModuleRegistry, exempt: &[&str]) -> Vec<String> {
    let mut misses = Vec::new();
    let mut types: Vec<&str> = registry.types().collect();
    types.sort_unstable();
    for type_id in types {
        if UNBUILDABLE.iter().any(|(id, _)| *id == type_id) {
            continue;
        }
        check_declared_keys(registry, type_id, &mut misses);
        if exempt.contains(&type_id) {
            if !registry.config_keys(type_id).is_empty() {
                misses.push(format!(
                    "{type_id} declares its config keys: take it off NOT_YET_MIGRATED"
                ));
            }
            continue;
        }
        check_control_keys(registry, type_id, &mut misses);
    }
    for type_id in exempt {
        if !registry.has_type(type_id) {
            misses.push(format!("{type_id} is not a registered type"));
        }
    }
    misses
}

#[test]
fn every_module_type_reads_its_config_numbers_by_the_reader() {
    let registry = ModuleRegistry::default();
    let misses = check_registry(&registry, NOT_YET_MIGRATED);
    assert!(misses.is_empty(), "{}", misses.join("\n"));
    assert!(!registry.config_keys("oscillator").is_empty());
}

#[test]
fn the_harness_checks_integer_keys() {
    let mut registry = ModuleRegistry::new();
    registry.register(ProbeFactory);
    assert_eq!(check_registry(&registry, &[]), Vec::<String>::new());
}

#[test]
fn the_harness_catches_a_factory_that_reads_numbers_its_own_way() {
    let mut registry = ModuleRegistry::new();
    registry.register(LegacyProbeFactory);
    let misses = check_registry(&registry, &[]);
    assert!(
        misses
            .iter()
            .any(|miss| miss.starts_with("legacy_probe.hz: 3.0 builds")),
        "{misses:#?}"
    );
    assert!(
        misses
            .iter()
            .any(|miss| miss.starts_with("legacy_probe.hz: 3.5 gave Ok")),
        "{misses:#?}"
    );
    // A migrated type left on the list is flagged.
    let mut registry = ModuleRegistry::new();
    registry.register(ProbeFactory);
    let misses = check_registry(&registry, &["probe"]);
    assert_eq!(
        misses,
        vec!["probe declares its config keys: take it off NOT_YET_MIGRATED"]
    );
}
