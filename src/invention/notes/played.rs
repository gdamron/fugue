//! The notes an engine on another thread plays, carried as numbers: a
//! browser player's AudioWorklet renders the piece, and the page beside it
//! keeps what was played.
//!
//! The rendering side drains its note feed without pairing it, so the
//! drain neither allocates, frees nor locks: [`write_words`] writes the
//! edges, and where each tapped module's clock stands, into a buffer the
//! host owns (see [`RenderEngine::take_note_words`]). The other side feeds
//! those words to [`PlayedNotes`], which pairs them as the live feed does
//! and keeps them in a [`NoteHistory`], so its pages are the daemon's.
//!
//! The words, all `f64`: a header of [`HEADER_WORDS`] (the run, edges the
//! ring dropped, keys given so far, then the taps and edges that follow),
//! a record of [`TAP_WORDS`] per tapped module (its key, its clock's key or
//! -1, and the beats that clock reached), and one of [`EDGE_WORDS`] per
//! edge (key, voice, on, frame, beats, clock key or -1, frequency,
//! velocity, after loss).
//!
//! [`RenderEngine::take_note_words`]: crate::RenderEngine::take_note_words

use super::history::NoteHistory;
use super::{NoteChanges, NoteEdge, NoteFeed, NoteTaps, Pairing, Playhead};
use crate::control_request::EventCursor;
use crate::invention::graph::notes::NOTE_EDGE_CAPACITY;
use crate::rpc::NotePage;
use crate::spsc::Consumer;

const HEADER_WORDS: usize = 5;
const TAP_WORDS: usize = 3;
const EDGE_WORDS: usize = 9;

/// The most tapped modules one drain reports: the notes of a module past
/// them are still read, but a page for it has no playhead.
const MAX_TAPS: usize = 256;

/// The length of a buffer that holds any drain: every tap, and as many
/// edges as the ring holds.
pub const NOTE_WORDS_LEN: usize =
    HEADER_WORDS + MAX_TAPS * TAP_WORDS + NOTE_EDGE_CAPACITY * EDGE_WORDS;

/// Writes the edges `feed` holds, popped through `edges` (its claimed
/// reader), and where `taps` stand, into `out` as words (see the module
/// docs), stopping where `out` is full; the edges left wait for the next
/// drain. Returns the words written, 0 when `out` cannot hold a header.
/// Allocation-, free- and lock-free.
pub(crate) fn write_words(
    feed: &NoteFeed,
    edges: &mut Consumer<NoteEdge>,
    taps: &NoteTaps,
    run: u64,
    missed: &mut EventCursor,
    out: &mut [f64],
) -> usize {
    if out.len() < HEADER_WORDS {
        return 0;
    }
    let key = |key: Option<u32>| key.map_or(-1.0, f64::from);
    let mut at = HEADER_WORDS;
    let mut tapped = 0;
    for tap in taps.0.iter().take(MAX_TAPS) {
        let Some(record) = out.get_mut(at..at + TAP_WORDS) else {
            break;
        };
        record.copy_from_slice(&[f64::from(tap.key), key(tap.clock_key), tap.last_beats()]);
        at += TAP_WORDS;
        tapped += 1;
    }
    let lost = feed.missed_since(missed);
    let mut written = 0;
    while out.len() >= at + EDGE_WORDS {
        let Some(edge) = edges.pop() else {
            break;
        };
        out[at..at + EDGE_WORDS].copy_from_slice(&[
            f64::from(edge.key),
            f64::from(edge.voice),
            f64::from(u8::from(edge.on)),
            edge.frame as f64,
            edge.beats,
            key(edge.clock),
            f64::from(edge.frequency),
            f64::from(edge.velocity),
            f64::from(u8::from(edge.after_loss)),
        ]);
        at += EDGE_WORDS;
        written += 1;
    }
    out[..HEADER_WORDS].copy_from_slice(&[
        run as f64,
        f64::from(lost),
        feed.keys().count() as f64,
        tapped as f64,
        written as f64,
    ]);
    at
}

/// The notes an engine elsewhere played, fed from its words, and read by
/// cursor as the daemon's are.
pub struct PlayedNotes {
    history: NoteHistory,
    pairing: Pairing,
    /// The id of each key, by key.
    names: Vec<String>,
    /// Each tapped module's key, its clock's key, and the beats that clock
    /// reached, as of the latest feed.
    taps: Vec<(u32, Option<u32>, f64)>,
    run: Option<u64>,
}

impl PlayedNotes {
    /// Nothing played yet; the first change is numbered `first_seq` (see
    /// [`NoteHistory::starting_at`]).
    pub fn starting_at(first_seq: u64) -> Self {
        Self {
            history: NoteHistory::starting_at(first_seq),
            pairing: Pairing::default(),
            names: Vec::new(),
            taps: Vec::new(),
            run: None,
        }
    }

    /// The id of each key, by key, as [`RenderEngine::note_names`] gives
    /// them. A key names one id for as long as the engine lives, across
    /// loads, and keys only grow: set them again when a feed's header
    /// counts more keys than [`Self::names_known`].
    ///
    /// [`RenderEngine::note_names`]: crate::RenderEngine::note_names
    pub fn set_names(&mut self, names: Vec<String>) {
        self.names = names;
    }

    pub fn names_known(&self) -> usize {
        self.names.len()
    }

    /// The keys a drain's `words` were written with: set names first if
    /// this is more than [`Self::names_known`].
    pub fn keys_in(words: &[f64]) -> usize {
        words.get(2).map_or(0, |&keys| keys as usize)
    }

    /// Takes one drain's words; an empty drain takes nothing. Refuses
    /// words it cannot read, and words naming keys it has no names for,
    /// with nothing taken.
    pub fn feed(&mut self, words: &[f64]) -> Result<(), String> {
        if words.is_empty() {
            return Ok(());
        }
        let [run, missed, keys, taps, edges] = match words.get(..HEADER_WORDS) {
            Some(&[a, b, c, d, e]) => [a, b, c, d, e],
            _ => return Err("note words hold no header".to_string()),
        };
        let (taps, edges) = (taps as usize, edges as usize);
        let tap_words = taps.saturating_mul(TAP_WORDS);
        let needed = edges
            .saturating_mul(EDGE_WORDS)
            .saturating_add(tap_words)
            .saturating_add(HEADER_WORDS);
        if words.len() < needed {
            return Err(format!(
                "note words are cut short: {} words for {taps} taps and {edges} edges",
                words.len()
            ));
        }
        if keys as usize > self.names.len() {
            return Err(format!(
                "note words name {keys} keys but {} names are known: set the names first",
                self.names.len()
            ));
        }
        let run = run as u64;
        if self.run != Some(run) {
            self.pairing = Pairing::default();
            self.run = Some(run);
        }
        let key = |word: f64| (word >= 0.0).then_some(word as u32);
        let body = &words[HEADER_WORDS..];
        self.taps = (body[..tap_words].chunks_exact(TAP_WORDS))
            .map(|tap| (tap[0] as u32, key(tap[1]), tap[2]))
            .collect();
        let names = &self.names;
        let mut notes = Vec::new();
        for word in body[tap_words..].chunks_exact(EDGE_WORDS).take(edges) {
            let edge = NoteEdge {
                key: word[0] as u32,
                voice: word[1] as u16,
                on: word[2] != 0.0,
                frame: word[3] as u64,
                beats: word[4],
                clock: key(word[5]),
                frequency: word[6] as f32,
                velocity: word[7] as f32,
                after_loss: word[8] != 0.0,
            };
            let name = |key: u32| names.get(key as usize).cloned();
            self.pairing.pair(&edge, name, &mut notes);
        }
        let missed = missed as u32;
        self.history.record(run, NoteChanges { notes, missed });
        Ok(())
    }

    /// The notes of `module_id` changed after `after`, as
    /// [`NoteHistory::page`] gives them, with the playhead of the clock its
    /// notes count on: at its start (0) before anything has rendered.
    pub fn page(&self, module_id: &str, after: Option<u64>, sample_rate: u32) -> NotePage {
        let name = |key: u32| self.names.get(key as usize);
        let playhead = (self.taps.iter())
            .find(|(key, ..)| name(*key).is_some_and(|id| id == module_id))
            .and_then(|&(_, clock, beats)| {
                Some(Playhead {
                    clock: name(clock?)?.clone(),
                    beats: if beats.is_nan() { 0.0 } else { beats },
                })
            });
        self.history.page(module_id, after, playhead, sample_rate)
    }
}

#[cfg(test)]
#[path = "played_tests.rs"]
mod tests;
