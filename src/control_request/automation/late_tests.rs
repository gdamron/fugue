//! A sample instrument taking its automation late (as a feedback group's
//! member takes a segment's schedule entries at its end): a note off and a
//! release written a sample after it, taken together, fade the note over
//! the release in force at the note off, as on time, whatever order the
//! writers ran in.

use super::take_automation;
use crate::control_request::hold::FrameScope;
use crate::control_request::RtValue;
use crate::factory::{GraphModule, ModuleFactory};
use crate::modules::sample_instrument::SampleInstrumentFactory;
use crate::Module;

/// Held `notes`, each at 0.4, under a 1 ms release, the file to remove.
fn held_notes(notes: &[f32]) -> (Box<dyn Module + Send>, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "fugue-late-automation-{}-{:?}.wav",
        std::process::id(),
        std::thread::current().id()
    ));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for _ in 0..44_100 {
        writer.write_sample((0.4 * i16::MAX as f32) as i16).unwrap();
    }
    writer.finalize().unwrap();
    let config = serde_json::json!({
        "release": 0.001,
        "zones": [ { "root_note": 60, "key_range": [0, 127], "asset": path.to_str().unwrap() } ]
    });
    let built = SampleInstrumentFactory.build(44_100, &config).unwrap();
    let GraphModule::Module(mut instrument) = built.module else {
        panic!("sample_instrument is not a sink");
    };
    for &note in notes {
        instrument.set_control("note_on", note).unwrap();
        instrument.process(1);
    }
    (instrument, path)
}

#[test]
fn a_release_written_a_sample_after_a_note_off_taken_late_does_not_reach_back() {
    let (mut instrument, path) = held_notes(&[60.0]);
    {
        let (table, cells) = instrument.declared().unwrap();
        let (off, release) = (table.resolve("note_off"), table.resolve("release"));
        let slots = &cells.automation;
        {
            let _at = FrameScope::enter(10);
            slots.write(off.unwrap(), RtValue::F32(60.0), true);
        }
        let _at = FrameScope::enter(11);
        slots.write(release.unwrap(), RtValue::F32(1.0), false);
    }
    take_automation(instrument.as_mut());
    for _ in 0..64 {
        instrument.process(1);
    }
    let heard = instrument.get_output("audio_left").unwrap();
    let _ = std::fs::remove_file(path);
    assert!(heard.abs() < 2e-3, "the old release ended it: {heard}");
}

/// Two schedulers in separate feedback groups, each writing its whole
/// segment: the first a note off at 10 and another at 12, the second a
/// note off at 10 and a release at 12. Both notes off at 10 fade over the
/// release in force there.
#[test]
fn note_offs_written_out_of_sample_order_fade_under_their_samples_release() {
    let (mut instrument, path) = held_notes(&[60.0, 64.0]);
    {
        let (table, cells) = instrument.declared().unwrap();
        let (off, release) = (table.resolve("note_off"), table.resolve("release"));
        let (off, release) = (off.unwrap(), release.unwrap());
        let slots = &cells.automation;
        let write = |at, index, note, event| {
            let _at = FrameScope::enter(at);
            slots.write(index, RtValue::F32(note), event);
        };
        write(10, off, 60.0, true);
        write(12, off, 60.0, true);
        write(10, off, 64.0, true);
        write(12, release, 1.0, false);
    }
    take_automation(instrument.as_mut());
    for _ in 0..64 {
        instrument.process(1);
    }
    let heard = instrument.get_output("audio_left").unwrap();
    let _ = std::fs::remove_file(path);
    assert!(heard.abs() < 2e-3, "a later release reached back: {heard}");
}
