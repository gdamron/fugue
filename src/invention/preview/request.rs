//! What a preview is asked for, read from its request JSON, and the cursor
//! that carries a paged answer on.

use serde_json::{Map, Value};

/// The bars a request without `bars` spans.
pub const DEFAULT_BARS: u64 = 4;

/// The most bars one request may span.
pub const MAX_BARS: u64 = 64;

/// The largest seed a request may give: JavaScript's largest safe integer,
/// so a seed survives a round trip through a JS number.
const MAX_SEED: u64 = (1 << 53) - 1;

/// A parsed request.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Request {
    pub(super) bars: u64,
    pub(super) start_bar: u64,
    pub(super) module_id: Option<String>,
    pub(super) seed: u64,
    pub(super) cursor: Option<Cursor>,
}

/// Where the next page starts, and the request it belongs to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Cursor {
    pub(super) offset: usize,
    fingerprint: u64,
}

const CURSOR_PREFIX: &str = "notes1.";

impl Cursor {
    pub(super) fn new(offset: usize, fingerprint: u64) -> Self {
        Self {
            offset,
            fingerprint,
        }
    }

    pub(super) fn encode(self) -> String {
        format!("{CURSOR_PREFIX}{}.{:016x}", self.offset, self.fingerprint)
    }

    fn decode(text: &str) -> Option<Self> {
        let (offset, fingerprint) = text.strip_prefix(CURSOR_PREFIX)?.split_once('.')?;
        let offset = offset.parse().ok().filter(|offset| *offset > 0)?;
        if fingerprint.len() != 16 {
            return None;
        }
        let fingerprint = u64::from_str_radix(fingerprint, 16).ok()?;
        Some(Self::new(offset, fingerprint))
    }

    /// Whether this cursor came from an answer to `fingerprint`'s request.
    pub(super) fn fits(&self, fingerprint: u64) -> bool {
        self.fingerprint == fingerprint
    }
}

impl Request {
    /// Reads a request, refusing anything malformed with an error naming
    /// the field.
    pub(super) fn parse(json: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(json)
            .map_err(|error| format!("the request is not JSON: {error}"))?;
        let Value::Object(fields) = value else {
            return Err("the request must be a JSON object".to_string());
        };
        if let Some(unknown) = fields.keys().find(|key| {
            !["bars", "start_bar", "module_id", "seed", "cursor"].contains(&key.as_str())
        }) {
            return Err(format!(
                "the request has an unknown field '{unknown}'; it takes bars, start_bar, module_id, seed and cursor"
            ));
        }
        let bars = whole(&fields, "bars")?.unwrap_or(DEFAULT_BARS);
        if bars == 0 {
            return Err("bars is 0; it must be at least 1".to_string());
        }
        if bars > MAX_BARS {
            return Err(format!("bars is {bars}; the largest allowed is {MAX_BARS}"));
        }
        let start_bar = whole(&fields, "start_bar")?.unwrap_or(0);
        if start_bar > u64::from(u32::MAX) {
            return Err(format!(
                "start_bar is {start_bar}; the largest allowed is {}",
                u32::MAX
            ));
        }
        let seed = whole(&fields, "seed")?.unwrap_or(0);
        if seed > MAX_SEED {
            return Err(format!("seed is {seed}; the largest allowed is {MAX_SEED}"));
        }
        let module_id = text(&fields, "module_id")?;
        let cursor = match text(&fields, "cursor")? {
            None => None,
            Some(cursor) => Some(Cursor::decode(&cursor).ok_or_else(|| {
                "cursor is not one a previous answer gave; pass the cursor string as it came back"
                    .to_string()
            })?),
        };
        Ok(Self {
            bars,
            start_bar,
            module_id,
            seed,
            cursor,
        })
    }

    /// Identifies the request a cursor belongs to: the document, the sample
    /// rate and every field but the cursor (FNV-1a, stable everywhere).
    pub(super) fn fingerprint(&self, document: &str, sample_rate: u32) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |bytes: &[u8]| {
            for byte in bytes {
                hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
            }
            // A separator, so adjacent fields cannot run together.
            hash = (hash ^ 0xff).wrapping_mul(0x0000_0100_0000_01b3);
        };
        eat(document.as_bytes());
        eat(&sample_rate.to_le_bytes());
        eat(&self.bars.to_le_bytes());
        eat(&self.start_bar.to_le_bytes());
        eat(&self.seed.to_le_bytes());
        match &self.module_id {
            Some(id) => eat(format!("m{id}").as_bytes()),
            None => eat(b"-"),
        }
        hash
    }
}

/// A whole number field from 0 to 2^64-1, or `None` when absent.
fn whole(fields: &Map<String, Value>, name: &str) -> Result<Option<u64>, String> {
    match fields.get(name) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{name} must be a whole number of 0 or more, not {value}")),
    }
}

/// A string field, or `None` when absent.
fn text(fields: &Map<String, Value>, name: &str) -> Result<Option<String>, String> {
    match fields.get(name) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(value) => Err(format!("{name} must be a string, not {value}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_defaults_and_refuses_each_bad_field_by_name() {
        let request = Request::parse(r#"{"bars":8}"#).unwrap();
        assert_eq!((request.bars, request.start_bar, request.seed), (8, 0, 0));
        assert_eq!(request.module_id, None);
        let bare = Request::parse("{}").unwrap();
        assert_eq!(bare.bars, DEFAULT_BARS, "four bars when none are asked for");
        for (json, field) in [
            (r#"{"bars":0}"#, "bars"),
            (r#"{"bars":1.5}"#, "bars"),
            (r#"{"bars":-1}"#, "bars"),
            (r#"{"bars":1,"start_bar":"x"}"#, "start_bar"),
            (r#"{"bars":1,"seed":9007199254740992}"#, "seed"),
            (r#"{"bars":1,"module_id":7}"#, "module_id"),
            (r#"{"bars":1,"cursor":""}"#, "cursor"),
            (r#"{"bars":1,"tempo":3}"#, "tempo"),
        ] {
            let error = Request::parse(json).unwrap_err();
            assert!(error.contains(field), "{json}: {error}");
        }
        assert_eq!(
            Request::parse(r#"{"bars":65}"#).unwrap_err(),
            "bars is 65; the largest allowed is 64"
        );
        assert!(Request::parse("[1]").is_err());
    }

    #[test]
    fn a_cursor_round_trips_and_fits_only_its_request() {
        let request = Request::parse(r#"{"bars":8,"seed":3}"#).unwrap();
        let fingerprint = request.fingerprint("{}", 44_100);
        let text = Cursor::new(256, fingerprint).encode();
        let again = Request::parse(&format!(r#"{{"bars":8,"seed":3,"cursor":"{text}"}}"#)).unwrap();
        let cursor = again.cursor.unwrap();
        assert_eq!(cursor.offset, 256);
        assert!(cursor.fits(again.fingerprint("{}", 44_100)));
        let other = Request::parse(r#"{"bars":8,"seed":4}"#).unwrap();
        assert!(!cursor.fits(other.fingerprint("{}", 44_100)));
        assert!(!cursor.fits(request.fingerprint("{}", 48_000)));
    }
}
