//! Where the table's reads may run, and what a panicking writer leaves.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::Ordering;

use super::super::MAX_DEGREES;
use crate::audio_thread::AudioThreadScope;
use crate::modules::melody::MelodyControls;
use crate::traits::ControlSurface;

#[test]
fn a_scheduled_read_through_get_control_runs_on_the_audio_thread() {
    // A scheduler ramping `degree.N` with the default `get_scheduled`
    // reaches `position_or_wait` from the audio thread; its lock-free try
    // must not assert it is a control thread.
    let controls = MelodyControls::new(60, vec![0, 2, 4]);
    let _audio = AudioThreadScope::enter();
    assert_eq!(controls.get_scheduled("degree.1"), Ok(2.0));
}

#[test]
fn a_writer_panicking_mid_landing_leaves_no_landing_count() {
    let controls = MelodyControls::new(60, vec![0, 2]);
    let table = &controls.table;
    let landed = catch_unwind(AssertUnwindSafe(|| {
        table.landing_as_one(|| panic!("a writer panicking mid-restore"))
    }));
    assert!(landed.is_err());
    assert_eq!(table.landing.load(Ordering::SeqCst), 0);
    let (mut d, mut w) = ([0; MAX_DEGREES], [0.0; MAX_DEGREES]);
    assert_eq!(table.read_into(&mut d, &mut w), Some(2));
}
