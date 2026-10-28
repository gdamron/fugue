//! A sample instrument taking its automation late (as a feedback group's
//! member takes a segment's schedule entries at its end): a note off and a
//! release written a sample after it, taken together, fade the note over
//! the release in force at the note off, as on time.

use super::take_automation;
use crate::control_request::hold::FrameScope;
use crate::control_request::RtValue;
use crate::factory::{GraphModule, ModuleFactory};
use crate::modules::sample_instrument::SampleInstrumentFactory;
use crate::Module;

/// A held note at 0.4 under a 1 ms release, its file to remove.
fn held_note() -> (Box<dyn Module + Send>, std::path::PathBuf) {
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
    instrument.set_control("note_on", 60.0).unwrap();
    instrument.process(1);
    (instrument, path)
}

#[test]
fn a_release_written_a_sample_after_a_note_off_taken_late_does_not_reach_back() {
    let (mut instrument, path) = held_note();
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
