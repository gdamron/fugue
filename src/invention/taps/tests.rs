//! Opening taps and reading them, with bins sent by hand as the audio
//! thread would send them.

use super::*;
use crate::invention::graph::taps::{TAP_BIN, TAP_BIN_CAPACITY};
use crate::invention::orchestration::ModulePorts;
use crate::invention::publish::MirrorModule;
use crate::invention::timelines::Flow;
use crate::spsc::Producer;

const INSTANCE: u32 = 7;

/// A mirror running `module` with `outputs`, as instance [`INSTANCE`].
fn mirror(module: &str, outputs: &[&str]) -> TopologyMirror {
    let mut mirror = TopologyMirror::default();
    let ports = ModulePorts {
        inputs: Vec::new(),
        outputs: outputs.iter().map(|port| port.to_string()).collect(),
    };
    let module_mirror = MirrorModule {
        ports,
        is_sink: false,
        flow: Flow::Plain,
        instance: INSTANCE,
    };
    mirror.modules.insert(module.to_string(), module_mirror);
    mirror
}

/// A tap on `osc`'s `audio` at 48 kHz, and the audio thread's end of its
/// ring.
fn opened() -> (Arc<TapRegistry>, OutputTap, Producer<TapBin>) {
    let registry = TapRegistry::new();
    let mut tap = (registry.open(&mirror("osc", &["audio"]), "osc", &[])).unwrap();
    tap.sample_rate = 48_000;
    let producer = Producer::claim(Arc::clone(&registry.board.slots[tap.slot].ring));
    (registry, tap, producer)
}

/// Sends the bins starting at each of `starts`, bin `n` spanning `-n..n`.
fn send(tap: &OutputTap, producer: &mut Producer<TapBin>, starts: impl IntoIterator<Item = u64>) {
    for start in starts {
        let n = (start / TAP_BIN as u64) as f32;
        let bin = TapBin {
            serial: tap.serial,
            start,
            min: [-n, 0.0],
            max: [n, 0.0],
        };
        producer.push(bin).unwrap();
    }
}

fn bins(from: u64, count: u64) -> impl Iterator<Item = u64> {
    (from..from + count).map(|n| n * TAP_BIN as u64)
}

#[test]
fn a_read_merges_bins_to_the_width_asked_and_keeps_the_remainder() {
    let (_registry, mut tap, mut producer) = opened();
    send(&tap, &mut producer, bins(10, 10));
    // 100 samples a bin rounds up to four of the audio thread's.
    let read = tap.read(100);
    assert_eq!(read.samples_per_bin, 128);
    assert_eq!((read.origin_frame, read.lost, read.open), (320, 0, true));
    assert_eq!(read.ports, ["audio"]);
    assert_eq!(read.channels.len(), 1);
    assert_eq!(read.channels[0].min, [-13.0, -17.0]);
    assert_eq!(read.channels[0].max, [13.0, 17.0]);
    assert_eq!(read.origin_seconds(), 320.0 / 48_000.0);

    // The two left over lead the next read.
    send(&tap, &mut producer, bins(20, 2));
    let read = tap.read(128);
    assert_eq!((read.origin_frame, read.lost, read.len()), (18 * 32, 0, 1));
    assert_eq!(read.channels[0].max, [21.0]);
}

#[test]
fn a_gap_drops_what_came_before_it_and_counts_what_was_skipped() {
    let (_registry, mut tap, mut producer) = opened();
    send(&tap, &mut producer, bins(0, 2));
    assert_eq!(tap.read(32).len(), 2);
    // Bins 2..5 sent, then 5 and 6 lost to a full ring.
    send(&tap, &mut producer, bins(2, 3).chain(bins(7, 3)));
    let read = tap.read(32);
    assert_eq!((read.origin_frame, read.len()), (7 * 32, 3));
    assert_eq!(read.lost, 5 * 32, "bins 2 to 6");
}

#[test]
fn a_read_returns_at_most_the_newest_max_bins() {
    let (_registry, mut tap, mut producer) = opened();
    let sent = TAP_BIN_CAPACITY as u64;
    send(&tap, &mut producer, bins(0, 1));
    tap.read(32);
    send(&tap, &mut producer, bins(1, sent));
    // 1024 bins merged in pairs is 512: under the cap, all of them.
    assert_eq!(tap.read(64).len(), 512);
    // At the finest width, a long stall past the cap keeps the newest.
    tap.pending = bins(1_025, MAX_TAP_BINS as u64 + 3)
        .map(|start| TapBin {
            serial: tap.serial,
            start,
            min: [0.0; 2],
            max: [0.0; 2],
        })
        .collect();
    let read = tap.read(32);
    assert_eq!(read.len(), MAX_TAP_BINS);
    assert_eq!(read.origin_frame, (1_025 + 3) * 32);
    assert_eq!(read.lost, 3 * 32);
}

#[test]
fn a_bin_of_nothing_but_nan_reads_nan() {
    let (_registry, mut tap, mut producer) = opened();
    let empty = TapBin {
        serial: tap.serial,
        start: 0,
        min: [f32::INFINITY; 2],
        max: [f32::NEG_INFINITY; 2],
    };
    producer.push(empty).unwrap();
    let read = tap.read(32);
    assert!(read.channels[0].min[0].is_nan() && read.channels[0].max[0].is_nan());
}

#[test]
fn opening_checks_the_module_and_its_ports() {
    let registry = TapRegistry::new();
    let mirror = mirror("mix", &["audio_left", "audio_right", "level"]);
    let error = |ports: &[&str]| registry.open(&mirror, "mix", ports).err().unwrap();
    assert!(matches!(
        registry.open(&mirror, "nope", &[]).err().unwrap(),
        GraphCommandError::UnknownModule(_)
    ));
    assert!(matches!(
        error(&["gate"]),
        GraphCommandError::InvalidPort(_)
    ));
    let three = ["audio_left", "audio_right", "level"];
    assert!(matches!(error(&three), GraphCommandError::InvalidPort(_)));
    let stereo = registry.open(&mirror, "mix", &[]).unwrap();
    assert_eq!(stereo.ports(), ["audio_left", "audio_right"]);
    assert_eq!(stereo.module_id(), "mix");
    let level = registry.open(&mirror, "mix", &["level"]).unwrap();
    assert_eq!(level.ports(), ["level"]);
}

#[test]
fn the_default_ports_are_the_audio() {
    let names = |outputs: &[&str]| {
        let outputs: Vec<String> = outputs.iter().map(|port| port.to_string()).collect();
        default_ports(&outputs)
    };
    let dac = names(&["audio", "audio_left", "audio_right"]);
    assert_eq!(dac, ["audio_left", "audio_right"]);
    assert_eq!(names(&["audio"]), ["audio"]);
    assert_eq!(names(&["frequency", "gate"]), ["frequency"]);
    assert!(names(&[]).is_empty());
}

#[test]
fn at_most_the_cap_are_open_and_dropping_one_frees_its_slot() {
    let registry = TapRegistry::new();
    let mirror = mirror("osc", &["audio"]);
    let mut taps: Vec<OutputTap> = (0..MAX_OUTPUT_TAPS)
        .map(|_| registry.open(&mirror, "osc", &[]).unwrap())
        .collect();
    let refused = registry.open(&mirror, "osc", &[]).err().unwrap();
    assert!(matches!(refused, GraphCommandError::ControlError(_)));
    taps.pop();
    let reopened = registry.open(&mirror, "osc", &[]).unwrap();
    assert_eq!(reopened.slot, MAX_OUTPUT_TAPS - 1);
}

#[test]
fn a_closed_tap_drops_the_remainder_it_kept_back() {
    let (registry, mut tap, mut producer) = opened();
    send(&tap, &mut producer, bins(0, 2));
    assert!(tap.read(128).is_empty(), "two bins wait for two more");
    let mut rebuilt = mirror("osc", &["audio"]);
    rebuilt.modules["osc"].instance = INSTANCE + 1;
    registry.close_gone(&rebuilt);
    let read = tap.read(32);
    assert!(!read.open && read.is_empty(), "{read:?}");
}

#[test]
fn a_tap_on_an_instance_no_longer_running_closes_and_reads_nothing_more() {
    let (registry, mut tap, mut producer) = opened();
    send(&tap, &mut producer, bins(0, 1));
    let mut rebuilt = mirror("osc", &["audio"]);
    rebuilt.modules["osc"].instance = INSTANCE + 1;
    registry.close_gone(&rebuilt);
    let read = tap.read(32);
    assert!(!read.open);
    // Its slot is free again; a bin a later tap sends there is not this
    // one's, and neither are bins this one sent before it closed.
    let mut later = registry.open(&rebuilt, "osc", &[]).unwrap();
    assert_eq!(later.slot, tap.slot);
    send(&later, &mut producer, bins(5, 1));
    assert!(tap.read(32).is_empty());
    assert_eq!(later.read(32).origin_frame, 5 * 32);
    // Dropping the closed tap leaves the later one open.
    drop(tap);
    assert!(later.read(32).open);
}
