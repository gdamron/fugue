//! Offline renders of every playable example under an invention seed, with
//! agent backends stubbed: seeded renders repeat, unseeded ones still vary,
//! and no agent reaches a real backend.

#[path = "../src/example_catalog.rs"]
#[allow(dead_code)]
mod example_catalog;

use std::thread;
use std::time::{Duration, Instant};

use example_catalog::{playable_examples, ExampleDefinition};
use fugue::{ControlValue, Invention, RenderEngine};

const SAMPLE_RATE: u32 = 48_000;
/// Long enough to tell a repeat from a change.
const SECONDS: usize = 3;
/// Long enough that two unseeded performances of the slowest generative
/// example (a melody at 60 bpm) pick a dozen notes, so they all but never
/// match by chance.
const PROBE_SECONDS: usize = 12;
/// Examples whose `code` or `agent` modules run on wall-clock time, so no
/// seed makes their renders repeat.
const WALL_CLOCK: [&str; 1] = ["in_c.json"];

fn engine(example: ExampleDefinition, seed: Option<u64>) -> RenderEngine {
    let path = example.path();
    let invention = Invention::from_file(path.to_str().unwrap())
        .unwrap_or_else(|err| panic!("{}: {err}", example.file_name));
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.set_seed(seed);
    engine.stub_agent_backends(true);
    engine
        .load_invention(invention)
        .unwrap_or_else(|err| panic!("{}: {err}", example.file_name));
    engine
}

/// The rendered samples' bits, folded into one FNV-1a hash.
fn render(example: ExampleDefinition, seed: Option<u64>) -> u64 {
    render_for(example, seed, SECONDS)
}

fn render_for(example: ExampleDefinition, seed: Option<u64>, seconds: usize) -> u64 {
    let mut engine = engine(example, seed);
    let mut block = vec![0.0f32; 1024];
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for _ in 0..(SAMPLE_RATE as usize * seconds).div_ceil(512) {
        engine.render_interleaved(&mut block).unwrap();
        for sample in &block {
            for byte in sample.to_bits().to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
    hash
}

/// Runs `check` over every playable example on a few threads.
fn each_example(check: impl Fn(ExampleDefinition) + Sync) {
    let examples = playable_examples();
    assert!(!examples.is_empty(), "the catalog lists examples");
    let threads = thread::available_parallelism().map_or(2, |n| n.get().min(4));
    let chunk = examples.len().div_ceil(threads);
    thread::scope(|scope| {
        for part in examples.chunks(chunk) {
            let check = &check;
            scope.spawn(move || part.iter().for_each(|example| check(*example)));
        }
    });
}

#[test]
fn a_seeded_render_of_every_example_repeats() {
    each_example(|example| {
        if WALL_CLOCK.contains(&example.file_name) {
            return;
        }
        let first = render(example, Some(1));
        let again = render(example, Some(1));
        assert_eq!(first, again, "{} varies under seed 1", example.file_name);
    });
}

#[test]
fn unseeded_examples_still_vary_and_each_seed_plays_its_own_performance() {
    let varying = std::sync::Mutex::new(Vec::new());
    each_example(|example| {
        if WALL_CLOCK.contains(&example.file_name) {
            return;
        }
        let name = example.file_name;
        let render = |seed| render_for(example, seed, PROBE_SECONDS);
        let unseeded = render(None);
        if unseeded == render(None) {
            // Nothing random is left unseeded: a seed changes nothing.
            let seeded = render(Some(1));
            assert_eq!(seeded, unseeded, "{name} moves under a seed");
            return;
        }
        varying.lock().unwrap().push(name);
        let (one, two) = (render(Some(1)), render(Some(2)));
        assert_ne!(one, two, "{name} plays alike under seeds 1 and 2");
    });
    let mut varying = varying.into_inner().unwrap();
    varying.sort_unstable();
    // The examples whose melody modules leave the seed out, at the top
    // level or inside a development.
    assert_eq!(
        varying,
        [
            "development_file_patch.json",
            "development_inline_patch.json",
            "developments/voice_library_trio.json",
            "filter_envelope.json",
            "filter_lfo_wah.json",
            "lfo_tremolo_sync.json",
            "lfo_vibrato.json",
            "mixer_voices.json",
            "modular_adsr_melody.json",
        ]
    );
}

#[test]
fn an_example_agent_contacts_no_backend_in_a_test_render() {
    let in_c = example_catalog::resolve_example("in_c.json").unwrap();
    let mut engine = engine(in_c, Some(1));
    let mut block = vec![0.0f32; 1024];
    let deadline = Instant::now() + Duration::from_secs(60);
    // The conductor is triggered every four beats; render until its worker
    // has answered one trigger.
    let error = loop {
        engine.render_interleaved(&mut block).unwrap();
        let error = match engine.get_control("conductor", "last_error") {
            Ok(ControlValue::String(error)) => error,
            other => panic!("conductor last_error: {other:?}"),
        };
        if !error.is_empty() || Instant::now() > deadline {
            break error;
        }
    };
    assert_eq!(
        error,
        "agent backends are stubbed in this render; 'local:codex' was not contacted"
    );
    let count = engine.get_control("conductor", "request_count").unwrap();
    assert_eq!(count, ControlValue::Number(0.0));
}
