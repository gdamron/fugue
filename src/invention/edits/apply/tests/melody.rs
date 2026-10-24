//! A batch adding a melody and writing within its scale: the writes bind
//! to the new module in the order made, each as written.

use serde_json::json;

use super::real_modules::assert_document_rebuilds_the_live_melody;
use super::{add, number, set, start, BASE};
use crate::ControlValue;

#[test]
fn an_added_melody_binds_a_whole_scale_and_the_writes_after_it_in_order() {
    let (mut running, pump) = start(BASE);
    running
        .apply_edits(&[
            add("tune", "melody", json!({ "degrees": [0, 2, 4] })),
            set("tune", "degree.1", number(9.0)),
            set("tune", "degrees", ControlValue::String("[0, 5, 7]".into())),
            set("tune", "degree.1", number(4.0)),
            set("tune", "degree_count", number(5.0)),
        ])
        .expect("the batch commits");
    pump.render(1);
    assert_eq!(
        running.get_control("tune", "degree.1").unwrap(),
        number(4.0)
    );
    assert_eq!(
        running.get_control("tune", "degree_count").unwrap(),
        number(5.0)
    );
    assert_eq!(
        running.get_control("tune", "degree.3").unwrap(),
        number(0.0)
    );
    assert_document_rebuilds_the_live_melody(&running);
}
