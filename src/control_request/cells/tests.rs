//! Cells hold every kind of value whole, and publishing one never
//! allocates.

use super::*;
use crate::alloc_counter::allocator_events;

#[test]
fn cells_hold_every_kind_and_publish_without_allocating() {
    let values = [
        RtValue::F32(-0.0),
        RtValue::U32(u32::MAX),
        RtValue::I32(-7),
        RtValue::Bool(true),
    ];
    let cells = ControlCells::new(values.iter().map(|_| RtValue::Bool(false)));
    let ((), allocs, frees) = allocator_events(|| {
        for (index, value) in values.iter().enumerate() {
            cells.publish(ControlIndex(index as u16), *value);
        }
        cells.publish(ControlIndex(9), RtValue::F32(1.0));
    });
    assert_eq!((allocs, frees), (0, 0));
    for (index, value) in values.iter().enumerate() {
        let loaded = cells.load(ControlIndex(index as u16)).unwrap();
        // Bit for bit: -0.0 stays negative.
        assert_eq!(format!("{loaded:?}"), format!("{value:?}"));
    }
    assert_eq!(cells.load(ControlIndex(4)), None);
}
