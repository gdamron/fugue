//! A non-finite control number is refused before it is sent: JSON cannot
//! carry it.

use super::*;

fn set(value: f32) -> StructuralEdit {
    StructuralEdit::SetControl {
        module_id: "osc".into(),
        key: "frequency".into(),
        value: ControlValue::Number(value),
    }
}

#[test]
fn a_non_finite_number_is_refused_at_its_edit() {
    // A value too large for an f32 becomes an infinity when converted.
    let overflow = 1e39_f64 as f32;
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, overflow] {
        let error = check_edit_batch(&[add("a"), set(440.0), set(value)]).unwrap_err();
        assert_eq!(error.code, RpcErrorCode::InvalidEdit, "{value}");
        let edit = error.edit.expect("a per-edit refusal");
        assert_eq!(
            (edit.index, edit.op, edit.reason),
            (
                2,
                EditOp::SetControl,
                EditFailureReason::InvalidControlValue
            ),
            "{value}"
        );
        assert!(
            edit.message
                .contains("'osc.frequency' expects a finite number"),
            "{}",
            edit.message
        );
        assert!(
            edit.message.ends_with(&value.to_string()),
            "{}",
            edit.message
        );
    }
}

#[test]
fn the_first_non_finite_number_is_the_one_refused() {
    let error = check_edit_batch(&[set(f32::NAN), set(f32::INFINITY)]).unwrap_err();
    assert_eq!(error.edit.unwrap().index, 0);
}

#[test]
fn finite_numbers_pass() {
    assert!(check_edit_batch(&[set(f32::MAX), set(f32::MIN), set(0.0)]).is_ok());
}

#[test]
fn batch_size_is_checked_before_any_value() {
    let over: Vec<StructuralEdit> = (0..=MAX_EDITS_PER_BATCH).map(|_| set(f32::NAN)).collect();
    let error = check_edit_batch(&over).unwrap_err();
    assert_eq!(error.code, RpcErrorCode::InvalidRequest);
    assert!(error.edit.is_none());
}
