//! The differences for a host that holds its documents as text, with no
//! daemon: the browser engine's `diffInventions`. The query, its limits and
//! its refusals are the daemon's own, so a page answers as `GetDiff` does.

use super::{invalid, DiffBaseline, DiffQuery, DiffSource, SoundingValue};
use crate::rpc::{RpcError, RpcErrorCode, RuntimeRevision};
use crate::Invention;

/// The texts one read takes, each JSON: the `baseline` document, as last
/// loaded or saved, and its kind (`"loaded"` or `"saved"`); the `current`
/// document; the controls `sounding` (`[{module_id, key, value}]`, the
/// values written as performances, as they sound); the host's `revision`
/// (`{session_id, revision}`); and the `query` (`limit`, `cursor`; `null`
/// or `{}` for the first page).
#[derive(Clone, Copy)]
pub struct DiffTexts<'a> {
    pub baseline: &'a str,
    pub kind: &'a str,
    pub current: &'a str,
    pub sounding: &'a str,
    pub revision: &'a str,
    pub query: &'a str,
}

/// One page of the differences, as compact JSON text: the daemon's
/// `Diff` page. Malformed JSON is `InvalidRequest`.
pub fn diff_documents(texts: &DiffTexts<'_>) -> Result<String, RpcError> {
    let revision: RuntimeRevision = parse("revision", texts.revision)?;
    let query: Option<DiffQuery> = parse("diff query", texts.query)?;
    let kind: DiffBaseline = serde_json::from_value(texts.kind.into())
        .map_err(|_| invalid("baseline kind must be \"loaded\" or \"saved\""))?;
    let sounding: Vec<SoundingValue> = parse("sounding values", texts.sounding)?;
    let document = |what: &str, text: &str| {
        Invention::from_json(text)
            .map_err(|error| invalid(&format!("invalid {what} JSON: {error}")))
    };
    let (baseline, current) = (
        document("baseline", texts.baseline)?,
        document("invention", texts.current)?,
    );
    let source = DiffSource {
        baseline: (&baseline, kind),
        current: &current,
        sounding: &sounding,
        revision: &revision,
    };
    let page = source.page(&query.unwrap_or_default())?;
    serde_json::to_string(&page).map_err(|e| RpcError::new(RpcErrorCode::Internal, e.to_string()))
}

fn parse<T: serde::de::DeserializeOwned>(what: &str, text: &str) -> Result<T, RpcError> {
    serde_json::from_str(text).map_err(|error| invalid(&format!("invalid {what} JSON: {error}")))
}
