//! Finding what Fugue offers from a few key words.
//!
//! One index holds a card for each thing an agent can use: a host's tools
//! and views, the module types, developments, examples and packages the
//! daemon knows, and the loaded piece's controls. [`Index::find`] ranks the
//! cards for a query by plain lexical matching over their names, labels,
//! descriptions and key words, widened by one table of musical synonyms
//! ([`synonyms`]). It is deterministic, needs no model and no network, and
//! bounds its answer in bytes, saying how many more matched when it stops.
//!
//! [`Gather`] reads the daemon's side of the index through the existing
//! discovery reads, one request at a time, so any client with a daemon
//! connection can build it; a host adds its own tools and views. See
//! `FIND.md` beside this file for the contract, including what a card holds
//! for each kind.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::rpc::{RpcError, RpcErrorCode};

mod entries;
mod gather;
mod rank;
pub mod synonyms;

pub use entries::{content_entries, control_entries, module_type_entries, package_entries};
pub use gather::Gather;

/// Version of the answer's shape and meaning.
pub const FIND_SCHEMA_VERSION: u32 = 1;
/// The most bytes an answer serializes to. Cards past it are counted in
/// [`FindAnswer::more`], never cut part way.
pub const MAX_FIND_BYTES: usize = 24_576;
/// Cards returned when a query names no limit.
pub const DEFAULT_FIND_LIMIT: usize = 5;
/// The most cards one query may ask for.
pub const MAX_FIND_LIMIT: usize = 20;
/// The longest query, in bytes.
pub const MAX_FIND_WORDS_BYTES: usize = 256;

/// What a card describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FindKind {
    Control,
    ModuleType,
    Tool,
    View,
    Development,
    Example,
    Package,
}

impl FindKind {
    /// Every kind, in the order ties between equally good cards fall.
    pub const ALL: [FindKind; 7] = [
        FindKind::Control,
        FindKind::ModuleType,
        FindKind::Tool,
        FindKind::View,
        FindKind::Development,
        FindKind::Example,
        FindKind::Package,
    ];

    /// The kind as a query and a card spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            FindKind::Control => "control",
            FindKind::ModuleType => "module_type",
            FindKind::Tool => "tool",
            FindKind::View => "view",
            FindKind::Development => "development",
            FindKind::Example => "example",
            FindKind::Package => "package",
        }
    }
}

/// A search: key words, and optionally one kind and a card limit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FindQuery {
    /// Key words or intent, e.g. "reverb", "make it brighter"
    pub words: String,
    /// Only this kind
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<FindKind>,
    /// Most cards, 1-20 (default 5)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

impl FindQuery {
    /// A query for `words` of any kind, with the default limit.
    pub fn words(words: impl Into<String>) -> Self {
        Self {
            words: words.into(),
            ..Self::default()
        }
    }

    /// Refuses an empty or oversized query or a limit out of range; a query
    /// that matches nothing is an answer, not an error.
    pub fn validate(&self) -> Result<usize, RpcError> {
        let invalid = |message: String| RpcError::new(RpcErrorCode::InvalidRequest, message);
        if self.words.trim().is_empty() {
            return Err(invalid("words must hold at least one word".into()));
        }
        if self.words.len() > MAX_FIND_WORDS_BYTES {
            return Err(invalid(format!(
                "words must be at most {MAX_FIND_WORDS_BYTES} bytes"
            )));
        }
        match self.limit {
            None => Ok(DEFAULT_FIND_LIMIT),
            Some(limit) if (1..=MAX_FIND_LIMIT).contains(&limit) => Ok(limit),
            Some(_) => Err(invalid(format!(
                "limit must be between 1 and {MAX_FIND_LIMIT}"
            ))),
        }
    }
}

/// One thing Fugue offers: what it is, when to use it, and how.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct Card {
    pub kind: FindKind,
    /// The exact name to use it by: a tool's or a module type's name, a
    /// view, a package or content id, or `module_id.key` for a control.
    pub name: String,
    /// What it is, in a few words.
    pub what: String,
    /// When to use it.
    pub when: String,
    /// How to use it: the exact call or document fragment, by kind (see
    /// `FIND.md`).
    pub how: Value,
}

/// A card and the extra words it is found by.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub card: Card,
    /// Words beyond the card's name, what and when that should find it,
    /// such as a module type's control keys and port names.
    pub terms: Vec<String>,
    /// Longer prose that should find it at the weight of its `when`, such
    /// as a module type's control descriptions.
    pub text: Vec<String>,
}

impl Entry {
    /// An entry for `card`, found by its own words alone.
    pub fn new(card: Card) -> Self {
        Self {
            card,
            terms: Vec::new(),
            text: Vec::new(),
        }
    }

    /// Adds words that should find this entry.
    pub fn with_terms(mut self, terms: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.terms.extend(terms.into_iter().map(Into::into));
        self
    }

    /// Adds prose that should find this entry, weighed as its `when`.
    pub fn with_text(mut self, text: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.text.extend(text.into_iter().map(Into::into));
        self
    }
}

/// The ranked cards for a query, bounded in count and bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct FindAnswer {
    pub schema_version: u32,
    /// Best first.
    pub cards: Vec<Card>,
    /// How many more cards matched than are listed.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: usize,
    /// What the reader should know: no piece is loaded, a source could not
    /// be read, why the list stopped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// When nothing matched: the nearest words the index holds, and the
    /// kinds that had a match when a kind filter excluded them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nearest: Option<Nearest>,
}

/// Where to look next when a query matched nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct Nearest {
    /// Words in the index spelled closest to the query's.
    pub terms: Vec<String>,
    /// Kinds the query matches without its kind filter; empty, and still
    /// present, when it matches none.
    #[serde(default)]
    pub kinds: Vec<FindKind>,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

/// The cards a search ranks, and what is known about where they came from.
#[derive(Debug, Clone, Default)]
pub struct Index {
    entries: Vec<Entry>,
    piece_loaded: bool,
    notes: Vec<String>,
    /// Notes past [`MAX_NOTES`], counted rather than kept.
    notes_dropped: usize,
    left_out: Vec<(FindKind, String)>,
}

/// The most notes an index keeps; the rest are counted.
const MAX_NOTES: usize = 8;
/// The longest note an index keeps; a longer one is cut at a character.
const MAX_NOTE_CHARS: usize = 240;

/// `note`, cut to [`MAX_NOTE_CHARS`] characters.
fn clipped(note: String) -> String {
    match note.char_indices().nth(MAX_NOTE_CHARS) {
        Some((at, _)) => format!("{}...", &note[..at]),
        None => note,
    }
}

impl Index {
    /// An empty index with no piece loaded.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one entry.
    pub fn add(&mut self, entry: Entry) {
        self.entries.push(entry);
    }

    /// Adds `entries`.
    pub fn extend(&mut self, entries: impl IntoIterator<Item = Entry>) {
        self.entries.extend(entries);
    }

    /// Records whether a piece is loaded, so a search that asks about
    /// controls can say when none is.
    pub fn set_piece_loaded(&mut self, loaded: bool) {
        self.piece_loaded = loaded;
    }

    /// Whether a piece was loaded when the index was built.
    pub fn piece_loaded(&self) -> bool {
        self.piece_loaded
    }

    /// Adds a note every answer carries, such as a source that could not
    /// be read. A note is cut to 240 characters, a repeat is dropped, and
    /// past eight notes the rest are only counted, so notes never crowd
    /// out the cards.
    pub fn note(&mut self, note: impl Into<String>) {
        let note = clipped(note.into());
        if self.notes.contains(&note) {
            return;
        }
        if self.notes.len() < MAX_NOTES {
            self.notes.push(note);
        } else {
            self.notes_dropped += 1;
        }
    }

    /// Records that this index holds no cards of `kind`, and why: a query
    /// for that kind, or one that matches nothing, carries `why` as a note.
    /// A kind keeps the first reason given, cut like a note.
    pub fn leave_out(&mut self, kind: FindKind, why: impl Into<String>) {
        if self.left_out.iter().any(|(left, _)| *left == kind) {
            return;
        }
        self.left_out.push((kind, clipped(why.into())));
    }

    /// The entries, in the order they were added.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Whether the index holds a card of `kind` named `name`.
    pub fn contains(&self, kind: FindKind, name: &str) -> bool {
        (self.entries.iter()).any(|entry| entry.card.kind == kind && entry.card.name == name)
    }

    /// The best cards for `query`, best first. Refuses only an invalid
    /// query; one that matches nothing answers with no cards and says
    /// where to look instead.
    pub fn find(&self, query: &FindQuery) -> Result<FindAnswer, RpcError> {
        let limit = query.validate()?;
        let ranked = rank::rank(&self.entries, &query.words);
        let matches: Vec<usize> = (ranked.iter())
            .map(|hit| hit.entry)
            .filter(|&at| {
                query
                    .kind
                    .is_none_or(|kind| self.entries[at].card.kind == kind)
            })
            .collect();
        let mut answer = FindAnswer {
            schema_version: FIND_SCHEMA_VERSION,
            cards: Vec::new(),
            more: 0,
            notes: self.notes.clone(),
            nearest: None,
        };
        if self.notes_dropped > 0 {
            (answer.notes).push(format!("{} more notes were left out.", self.notes_dropped));
        }
        if !self.piece_loaded && matches!(query.kind, None | Some(FindKind::Control)) {
            answer
                .notes
                .push("No piece is loaded, so no piece's controls were searched.".into());
        }
        for (kind, why) in &self.left_out {
            let asked = query.kind == Some(*kind) || (query.kind.is_none() && matches.is_empty());
            if asked && !answer.notes.contains(why) {
                answer.notes.push(why.clone());
            }
        }
        if matches.is_empty() {
            answer.nearest = Some(self.nearest(query, &ranked));
            answer.notes.push("Nothing matched.".into());
            return Ok(answer);
        }
        self.fill(&mut answer, &matches, limit, query.kind.is_none());
        Ok(answer)
    }

    /// Lists the cards at `matches` while they fit the limit and the byte
    /// cap, counting the rest and saying why the list stopped. With
    /// `fold_packages`, a package listed after the development or example
    /// it holds is left out as the same thing twice.
    fn fill(&self, answer: &mut FindAnswer, matches: &[usize], limit: usize, fold_packages: bool) {
        // Room for the notes the stop adds and for `more`.
        const STOP_NOTE_ROOM: usize = 256;
        let mut bytes = serialized_len(answer) + STOP_NOTE_ROOM;
        let mut stopped_by_bytes = false;
        let mut folded = 0;
        for &at in matches {
            let card = &self.entries[at].card;
            if fold_packages && card.kind == FindKind::Package && holds_content(answer, card) {
                folded += 1;
                continue;
            }
            if answer.cards.len() == limit {
                break;
            }
            // A comma between cards.
            let size = serialized_len(card) + 1;
            if bytes + size > MAX_FIND_BYTES {
                stopped_by_bytes = true;
                break;
            }
            bytes += size;
            answer.cards.push(card.clone());
        }
        answer.more = matches.len() - answer.cards.len() - folded;
        if stopped_by_bytes {
            answer.notes.push(format!(
                "{} more matched; the answer stops at {MAX_FIND_BYTES} bytes. Ask with a kind or a lower limit.",
                answer.more
            ));
        } else if answer.more > 0 {
            answer.notes.push(format!(
                "{} more matched; raise limit (up to {MAX_FIND_LIMIT}) or add words or a kind.",
                answer.more
            ));
        }
    }

    fn nearest(&self, query: &FindQuery, ranked: &[rank::Hit]) -> Nearest {
        let mut kinds: Vec<FindKind> = (ranked.iter())
            .map(|hit| self.entries[hit.entry].card.kind)
            .collect();
        kinds.sort();
        kinds.dedup();
        Nearest {
            terms: rank::nearest_terms(&self.entries, &query.words),
            kinds,
        }
    }
}

/// Whether `answer` already lists the development or example a package
/// card holds.
fn holds_content(answer: &FindAnswer, package: &Card) -> bool {
    (answer.cards.iter()).any(|card| {
        matches!(card.kind, FindKind::Development | FindKind::Example) && card.name == package.name
    })
}

fn serialized_len(value: &impl Serialize) -> usize {
    serde_json::to_vec(value).map_or(0, |bytes| bytes.len())
}

#[cfg(test)]
mod tests;
