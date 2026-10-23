//! A batch checked after a timed edit has landed but before anything has
//! adopted it.

use serde_json::json;

use super::{add, config_of, start, BASE};
use crate::invention::manual_backend::SAMPLE_RATE;
use crate::invention::publish::EditTime;

#[test]
fn a_batch_after_a_landed_timed_swap_reads_the_document_the_swap_left() {
    let (mut running, pump) = start(BASE);
    let square = json!({ "waveform": "square", "frequency": 440.0 });
    let at = EditTime::AtSample(64);
    let swap =
        running
            .live
            .swap_module_at(SAMPLE_RATE, "osc1", "oscillator", &square, true, at, None);
    swap.unwrap();
    // Landed on the audio thread; nothing has locked the publisher since.
    pump.render(4);
    let batch = [add("osc3", "oscillator", json!({ "frequency": 660.0 }))];
    running.apply_edits(&batch).unwrap();
    assert_eq!(config_of(&running, "osc1")["waveform"], "square");
}
