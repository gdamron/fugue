//! Inspection of a document a host holds as text, for a host with no daemon:
//! the browser engine's `inspectInvention`. The query, its limits and its
//! refusals are the daemon's own, so a page answers as `InspectInvention` does.

use super::{invalid, InspectionQuery};
use crate::rpc::{AuthoredSnapshot, RevisionConflict, RpcError, RpcErrorCode, RuntimeRevision};
use crate::Invention;

/// Reads one page of `document` (invention JSON text) at `revision` (a
/// `{session_id, revision}` token, as JSON text), as `query` (an
/// [`InspectionQuery`], as JSON text) asks. Returns the page as compact
/// JSON text, the daemon's `InventionInspection` page; it has no
/// `source_path`, which is null.
///
/// `expected_revision`, when given, is checked first, as a daemon checks a
/// request's: one other than `revision` is refused with the shared
/// [`RevisionConflict`]. Malformed JSON is `InvalidRequest`.
pub fn inspect_document(
    document: &str,
    revision: &str,
    query: &str,
    expected_revision: Option<&str>,
) -> Result<String, RpcError> {
    let revision: RuntimeRevision = parse("revision", revision)?;
    if let Some(expected) = expected_revision {
        let expected: RuntimeRevision = parse("expected_revision", expected)?;
        if let Some(conflict) = RevisionConflict::between(&expected, &revision) {
            return Err(RpcError::revision_conflict(conflict));
        }
    }
    let query: InspectionQuery = parse("inspection query", query)?;
    let document = Invention::from_json(document)
        .map_err(|error| invalid(&format!("invalid invention JSON: {error}")))?;
    let page = AuthoredSnapshot::new(document, revision)?.inspect(&query)?;
    serde_json::to_string(&page).map_err(|e| RpcError::new(RpcErrorCode::Internal, e.to_string()))
}

fn parse<T: serde::de::DeserializeOwned>(what: &str, text: &str) -> Result<T, RpcError> {
    serde_json::from_str(text).map_err(|error| invalid(&format!("invalid {what} JSON: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::{ConflictReason, InspectionPage, InspectionSelection};
    use serde_json::json;

    const DOCUMENT: &str = r#"{"modules":[{"id":"voice","type":"oscillator"},{"id":"out","type":"dac"}],
        "connections":[{"from":"voice","from_port":"out","to":"out","to_port":"left"}]}"#;
    const REVISION: &str = r#"{"session_id":"page","revision":2}"#;

    fn query(selection: serde_json::Value) -> String {
        json!({ "selection": selection }).to_string()
    }

    #[test]
    fn answers_as_the_daemon_snapshot_does() {
        let text =
            inspect_document(DOCUMENT, REVISION, &query(json!({"kind":"overview"})), None).unwrap();
        let page: InspectionPage = serde_json::from_str(&text).unwrap();
        let snapshot = AuthoredSnapshot::new(
            Invention::from_json(DOCUMENT).unwrap(),
            serde_json::from_str(REVISION).unwrap(),
        )
        .unwrap();
        let expected = snapshot
            .inspect(&serde_json::from_str(&query(json!({"kind":"overview"}))).unwrap())
            .unwrap();
        assert_eq!(page, expected);
        assert_eq!(text, serde_json::to_string(&expected).unwrap());
        assert!(page.source_path.is_none());
        assert!(text.contains(r#""source_path":null"#));
    }

    #[test]
    fn a_stale_or_foreign_expected_revision_is_refused_before_reading() {
        let q = query(json!({"kind":"value","pointer":"/connections"}));
        let current = Some(REVISION);
        assert!(inspect_document(DOCUMENT, REVISION, &q, current).is_ok());
        let stale = r#"{"session_id":"page","revision":1}"#;
        let error = inspect_document("not json", REVISION, &q, Some(stale)).unwrap_err();
        assert_eq!(error.code, RpcErrorCode::RevisionConflict);
        assert_eq!(
            error.conflict.unwrap().reason,
            ConflictReason::StaleRevision
        );
        let foreign = r#"{"session_id":"daemon","revision":2}"#;
        let error = inspect_document(DOCUMENT, REVISION, &q, Some(foreign)).unwrap_err();
        assert_eq!(
            error.conflict.unwrap().reason,
            ConflictReason::SessionReplaced
        );
    }

    #[test]
    fn a_cursor_from_another_revision_is_refused() {
        let q = json!({"selection":{"kind":"overview"},"limit":1});
        let first: InspectionPage = serde_json::from_str(
            &inspect_document(DOCUMENT, REVISION, &q.to_string(), None).unwrap(),
        )
        .unwrap();
        let cursor = first.next_cursor.unwrap();
        let mut next = q.clone();
        next["cursor"] = serde_json::to_value(&cursor).unwrap();
        assert!(inspect_document(DOCUMENT, REVISION, &next.to_string(), None).is_ok());
        let later = r#"{"session_id":"page","revision":3}"#;
        let error = inspect_document(DOCUMENT, later, &next.to_string(), None).unwrap_err();
        assert_eq!(error.code, RpcErrorCode::RevisionConflict);
    }

    #[test]
    fn unknown_names_and_malformed_input_are_refused_with_the_daemon_wording() {
        let missing = query(json!({"kind":"module","id":"missing"}));
        let local = AuthoredSnapshot::new(
            Invention::from_json(DOCUMENT).unwrap(),
            serde_json::from_str(REVISION).unwrap(),
        )
        .unwrap()
        .inspect(&InspectionQuery {
            selection: InspectionSelection::Module {
                scope: String::new(),
                id: "missing".into(),
            },
            limit: 50,
            cursor: None,
        })
        .unwrap_err();
        assert_eq!(
            inspect_document(DOCUMENT, REVISION, &missing, None).unwrap_err(),
            local
        );
        for (document, revision, query) in [
            ("{", REVISION, missing.as_str()),
            (DOCUMENT, "{}", missing.as_str()),
            (DOCUMENT, REVISION, r#"{"selection":{"kind":"everything"}}"#),
        ] {
            let error = inspect_document(document, revision, query, None).unwrap_err();
            assert_eq!(
                error.code,
                RpcErrorCode::InvalidRequest,
                "{}",
                error.message
            );
        }
    }
}
