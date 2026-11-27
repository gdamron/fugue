use super::tests::{sounding, CURRENT, SAVED};
use super::*;
use crate::rpc::{ConflictReason, RuntimeRevision, MAX_INSPECTION_BYTES};
use serde_json::json;

fn revision(n: u64) -> RuntimeRevision {
    RuntimeRevision {
        session_id: "s".into(),
        revision: n,
    }
}

fn read(
    saved: &str,
    current: &str,
    sounding: &[SoundingValue],
    query: &DiffQuery,
) -> Result<DiffPage, RpcError> {
    let (saved, current) = (
        Invention::from_json(saved).unwrap(),
        Invention::from_json(current).unwrap(),
    );
    DiffSource {
        baseline: (&saved, DiffBaseline::Saved),
        current: &current,
        sounding,
        revision: &revision(4),
    }
    .page(query)
}

fn limit(limit: usize, cursor: Option<DiffCursor>) -> DiffQuery {
    DiffQuery { limit, cursor }
}

#[test]
fn a_page_counts_the_changes_and_says_what_changed() {
    let page = read(SAVED, CURRENT, &[], &DiffQuery::default()).unwrap();
    assert_eq!(page.summary, "9 changes since the piece was last saved.");
    assert_eq!(
        (page.total_changes, page.total_performed, page.offset),
        (9, 0, 0)
    );
    assert_eq!(page.changes.len(), 9);
    assert!(page.performed.is_empty() && page.next_cursor.is_none());
    assert_eq!(
        (page.revision, page.baseline),
        (revision(4), DiffBaseline::Saved)
    );
}

#[test]
fn the_same_piece_says_so_in_one_sentence() {
    let page = read(SAVED, SAVED, &[], &DiffQuery::default()).unwrap();
    assert!(page.changes.is_empty() && page.performed.is_empty());
    assert_eq!(
        page.summary,
        "Nothing has changed since the piece was last saved."
    );
}

#[test]
fn performed_values_are_ones_a_save_would_not_keep() {
    let values = [
        sounding("voice", "waveform", json!("saw")),
        sounding("hat", "level", json!(0.5)),
    ];
    let page = read(SAVED, SAVED, &values, &DiffQuery::default()).unwrap();
    assert_eq!(page.performed.len(), 2);
    assert_eq!(
        page.summary,
        "The document has not changed since the piece was last saved. 2 values sound only \
         performed and not in the document; a save would not keep them."
    );
    let one = read(SAVED, CURRENT, &values[..1], &DiffQuery::default()).unwrap();
    assert!(one.summary.ends_with(
        "1 value sounds only performed and not in the document; a save would not keep it."
    ));
}

#[test]
fn pages_follow_inspection_limits_and_carry_the_revision() {
    let values = [sounding("voice", "waveform", json!("saw"))];
    let first = read(SAVED, CURRENT, &values, &limit(6, None)).unwrap();
    assert_eq!((first.changes.len(), first.performed.len()), (6, 0));
    let cursor = first.next_cursor.clone().unwrap();
    assert_eq!((&cursor.revision, cursor.offset), (&revision(4), 6));
    let rest = read(SAVED, CURRENT, &values, &limit(6, Some(cursor))).unwrap();
    assert_eq!(
        (rest.offset, rest.changes.len(), rest.performed.len()),
        (6, 3, 1)
    );
    assert!(rest.next_cursor.is_none());
    assert_eq!(rest.summary, first.summary);
    for wrong in [0, 101] {
        let refused = read(SAVED, CURRENT, &[], &limit(wrong, None)).unwrap_err();
        assert_eq!(refused.code, RpcErrorCode::InvalidRequest);
    }
    let stale = DiffCursor {
        revision: revision(3),
        ..first.next_cursor.clone().unwrap()
    };
    let refused = read(SAVED, CURRENT, &[], &limit(6, Some(stale))).unwrap_err();
    assert_eq!(refused.code, RpcErrorCode::RevisionConflict);
    assert_eq!(
        refused.conflict.unwrap().reason,
        ConflictReason::StaleRevision
    );
}

#[test]
fn a_page_stays_within_its_budget() {
    // Twelve values of 3000 bytes each fit one by one, not all on a page.
    let long = "x".repeat(3000);
    let piece = |value: &str| {
        let modules: Vec<_> = (0..12)
            .map(|n| json!({"id":format!("m{n}"),"type":"oscillator","config":{"label":value}}))
            .collect();
        json!({"modules":modules,"connections":[]}).to_string()
    };
    let first = read(&piece(""), &piece(&long), &[], &DiffQuery::default()).unwrap();
    assert!(serde_json::to_vec(&first).unwrap().len() <= MAX_INSPECTION_BYTES);
    assert!(first.changes.len() < 12 && first.next_cursor.is_some());
}

#[test]
fn the_text_entry_answers_as_the_typed_one() {
    let values = json!([{"module_id":"voice","key":"waveform","value":"saw"}]).to_string();
    let texts = DiffTexts {
        baseline: SAVED,
        kind: "saved",
        current: CURRENT,
        sounding: &values,
        revision: r#"{"session_id":"s","revision":4}"#,
        query: "null",
    };
    let text = diff_documents(&texts).unwrap();
    let values = [sounding("voice", "waveform", json!("saw"))];
    let expected = read(SAVED, CURRENT, &values, &DiffQuery::default()).unwrap();
    assert_eq!(text, serde_json::to_string(&expected).unwrap());
    let loaded = diff_documents(&DiffTexts {
        kind: "loaded",
        current: SAVED,
        sounding: "[]",
        ..texts
    })
    .unwrap();
    assert!(loaded.contains("Nothing has changed since the piece was loaded."));
    for broken in [
        DiffTexts {
            kind: "kept",
            ..texts
        },
        DiffTexts {
            query: r#"{"offset":1}"#,
            ..texts
        },
        DiffTexts {
            current: "{",
            ..texts
        },
    ] {
        assert_eq!(
            diff_documents(&broken).unwrap_err().code,
            RpcErrorCode::InvalidRequest
        );
    }
}

#[test]
fn a_cursor_is_refused_once_the_differences_move_at_the_same_revision() {
    let values = [sounding("voice", "waveform", json!("saw"))];
    let first = read(SAVED, CURRENT, &values, &limit(2, None)).unwrap();
    let cursor = first.next_cursor.unwrap();
    // The same read follows on.
    let same = read(SAVED, CURRENT, &values, &limit(2, Some(cursor.clone())));
    assert_eq!(same.unwrap().offset, 2);
    // A save (the baseline is now the current piece), or a value performed
    // back, moves what is measured but not the revision.
    for (saved, values) in [(CURRENT, &values[..]), (SAVED, &[][..])] {
        let refused = read(saved, CURRENT, values, &limit(2, Some(cursor.clone()))).unwrap_err();
        assert_eq!(refused.code, RpcErrorCode::InvalidRequest);
        assert!(refused.message.contains("read again from the first page"));
    }
}
