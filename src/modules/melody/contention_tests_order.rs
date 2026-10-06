//! Ordering corners of the deferred-write mailbox: a deposit that is
//! observed before it is flagged, and the mirror of hidden positions.

use super::*;
use crate::{ControlSurface, ControlValue};

fn table_degrees(ctrl: &MelodyControls) -> Vec<i32> {
    ctrl.table.lock().unwrap().degrees.clone()
}

fn table_weights(ctrl: &MelodyControls) -> Vec<f32> {
    ctrl.table.lock().unwrap().weights.clone()
}

#[test]
fn a_direct_write_supersedes_a_deposit_observed_before_it_was_flagged() {
    let ctrl = MelodyControls::new(60, vec![0, 2, 4, 5]);
    let held = ctrl.table.lock().unwrap();
    // Depositors publish their slots, then pause before flagging them...
    assert!(ctrl.pending.stage_degree(ctrl.pending.degrees_gen(), 0, 2));
    assert!(ctrl
        .pending
        .stage_weight(ctrl.pending.weights_gen(), 0, 2.0));
    ctrl.pending.stage_count(3);
    // ...and contended reads observe them.
    assert_eq!(ctrl.degree(0).unwrap(), 2);
    assert_eq!(ctrl.note_weight(0).unwrap(), 2.0);
    assert_eq!(ctrl.degree_count(), 3);
    drop(held);

    // Later direct writes find nothing flagged to drain on acquire.
    ctrl.set_degree(0, 4).unwrap();
    ctrl.set_note_weight(0, 4.0).unwrap();
    ctrl.set_degree_count(4);
    // The paused depositors flag; the next drain must restore nothing.
    ctrl.pending.mark();
    drop(ctrl.lock_table());

    assert_eq!(table_degrees(&ctrl), [4, 2, 4, 5]);
    assert_eq!(table_weights(&ctrl), [4.0, 1.0, 1.0, 1.0]);
}

#[test]
fn a_ramp_start_on_a_revealed_position_reads_what_it_would_play() {
    let ctrl = MelodyControls::new(60, vec![0, 2, 4, 5]);
    ctrl.set_degree(3, 11).unwrap();
    // Replacing the scale erases the override at the now-hidden position 3.
    ctrl.set_allowed_degrees(vec![0, 4]);

    let held = ctrl.table.lock().unwrap();
    ctrl.set_degree_count(4);
    // A scheduler ramp samples its start value through the control surface.
    assert_eq!(
        ctrl.get_control("degree.3").unwrap(),
        ControlValue::Number(4.0)
    );
    drop(held);

    assert_eq!(ctrl.allowed_degrees(), [0, 4, 0, 4]);
}
