//! Restoring written positions is one whole edit to a reader.

use std::cell::RefCell;
use std::rc::Rc;

use super::super::MAX_DEGREES;
use super::support::between_loads_after;
use crate::modules::melody::tests::without_waiting;
use crate::modules::melody::MelodyControls;

#[test]
fn a_copy_never_holds_half_of_one_restore() {
    without_waiting(|| {
        let controls = MelodyControls::new(60, vec![0, 0]);
        let table = controls.table.clone();
        let copied: Rc<RefCell<Option<Option<Vec<i32>>>>> = Rc::new(RefCell::new(None));
        let (reader, out) = (table.clone(), copied.clone());
        // Past the first write's hook before it lands, to its hook after:
        // slot 0 has landed and slot 1 is not yet written.
        between_loads_after(1, move || {
            let (mut d, mut w) = ([0; MAX_DEGREES], [0.0; MAX_DEGREES]);
            *out.borrow_mut() = Some(reader.read_into(&mut d, &mut w).map(|n| d[..n].to_vec()));
        });
        table.restore_written([(0, 5), (1, 7)], []);
        let seen = copied.borrow().clone().expect("the copy ran mid-restore");
        assert!(
            matches!(seen.as_deref(), None | Some([0, 0]) | Some([5, 7])),
            "kept a half-restored table: {seen:?}"
        );
        assert_eq!(table.degrees(), [5, 7]);
    });
}
