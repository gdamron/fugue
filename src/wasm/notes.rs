//! The notes a player plays, for its page: the engine in the AudioWorklet
//! follows them and drains them as numbers (`FugueEngine.takeNoteWordsInto`),
//! and the page feeds those to a `PlayedNotes`, which keeps the history the
//! daemon keeps and pages it alike (see `crate::invention::notes::played`).

use js_sys::Float64Array;
use wasm_bindgen::prelude::*;

use super::WasmFugueEngine;
use crate::{PlayedNotes, NOTE_WORDS_LEN};

#[wasm_bindgen(js_class = FugueEngine)]
impl WasmFugueEngine {
    #[wasm_bindgen(js_name = followNotes)]
    /// Follows the notes the piece plays from here on, and those of every
    /// piece loaded after. Rendering sounds the same either way. Allocates
    /// the drain's buffer: call it once, before playing.
    pub fn follow_notes(&mut self) {
        if self.note_words.is_empty() {
            self.note_words = vec![0.0; NOTE_WORDS_LEN];
        }
        self.inner.follow_notes();
    }

    #[wasm_bindgen(js_name = noteWordsLength)]
    /// The length of the buffer `takeNoteWordsInto` fills.
    pub fn note_words_length() -> usize {
        NOTE_WORDS_LEN
    }

    #[wasm_bindgen(js_name = takeNoteWordsInto)]
    /// Drains the notes played since the last read into `out` (length
    /// `noteWordsLength()`) as words a `PlayedNotes` takes, and returns how
    /// many it wrote: 0 when not following or with nothing loaded. Pass
    /// `out.subarray(0, written)` on. Allocates nothing on the wasm heap,
    /// so a player may call it from its audio thread between renders; the
    /// copy out makes one short-lived typed-array view in JS. Drain a few
    /// times a second or more while playing: past about a thousand starts
    /// and ends between drains, some are missed and readers resync.
    pub fn take_note_words_into(&mut self, out: &Float64Array) -> Result<usize, JsValue> {
        if out.length() as usize != NOTE_WORDS_LEN {
            return Err(JsValue::from_str(
                "note words buffer length must be noteWordsLength()",
            ));
        }
        let written = self.inner.take_note_words(&mut self.note_words);
        if written > 0 {
            out.subarray(0, written as u32)
                .copy_from(&self.note_words[..written]);
        }
        Ok(written)
    }

    #[wasm_bindgen(js_name = noteNames)]
    /// The module and clock ids the drained words name by number, by
    /// number: what `PlayedNotes.setNames` takes. Allocates: ask only when
    /// `PlayedNotes.keysIn(words)` exceeds `namesKnown()`.
    pub fn note_names(&self) -> Vec<String> {
        self.inner.note_names()
    }
}

#[wasm_bindgen(js_name = PlayedNotes)]
/// The notes a player played, fed from its engine's words, read by cursor
/// as the daemon's `get_notes` reads them.
pub struct WasmPlayedNotes {
    inner: PlayedNotes,
}

#[wasm_bindgen(js_class = PlayedNotes)]
impl WasmPlayedNotes {
    #[wasm_bindgen(constructor)]
    /// Nothing played yet. Sequence numbers start at `firstSeq` (a page
    /// passes `Date.now() * 1000`, as the daemon starts at its start time
    /// in microseconds), so a cursor from an earlier page is told to
    /// resync rather than mistaken for a current one.
    pub fn new(first_seq: f64) -> WasmPlayedNotes {
        WasmPlayedNotes {
            inner: PlayedNotes::starting_at(first_seq.max(0.0) as u64),
        }
    }

    #[wasm_bindgen(js_name = setNames)]
    /// The ids `FugueEngine.noteNames` gave.
    pub fn set_names(&mut self, names: Vec<String>) {
        self.inner.set_names(names);
    }

    #[wasm_bindgen(js_name = namesKnown)]
    pub fn names_known(&self) -> usize {
        self.inner.names_known()
    }

    #[wasm_bindgen(js_name = keysIn)]
    /// How many ids one drain's words name: set names first when it is
    /// more than `namesKnown()`.
    pub fn keys_in(words: &[f64]) -> usize {
        PlayedNotes::keys_in(words)
    }

    /// Takes one drain's words. Throws, taking nothing, on words it cannot
    /// read or that name ids it has no names for.
    pub fn feed(&mut self, words: &[f64]) -> Result<(), JsValue> {
        self.inner
            .feed(words)
            .map_err(|error| JsValue::from_str(&error))
    }

    /// One module's notes changed after `after` (none or 0: every one
    /// kept), as JSON in the shape of the daemon's `get_notes` page, with
    /// seconds counted at `sampleRate`.
    pub fn page(
        &self,
        module_id: &str,
        after: Option<f64>,
        sample_rate: u32,
    ) -> Result<String, JsValue> {
        let after = after.map(|after| after.max(0.0) as u64);
        let page = self.inner.page(module_id, after, sample_rate);
        serde_json::to_string(&page).map_err(|error| JsValue::from_str(&error.to_string()))
    }
}
