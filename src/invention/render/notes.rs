//! The notes an offline render plays, followed on request: the browser
//! player's AudioWorklet renders the piece and its page keeps what was
//! played (see `crate::invention::notes::played`).
//!
//! Following taps the render's graph as a live graph is tapped: the taps
//! read the rendered gate outputs after each segment and change nothing
//! the render writes. Each load starts a new run with a feed of its own,
//! and each edit taps its new topology (see `publish::edit_offline`).

use std::sync::Arc;

use super::RenderEngine;
use crate::control_request::EventCursor;
use crate::invention::graph::SignalGraph;
use crate::invention::notes::{self, NoteChanges, NoteFeed};
use crate::invention::publish::TopologyMirror;

/// The feed of a render that follows notes.
#[derive(Default)]
pub(super) struct Followed {
    /// `None` until a piece is loaded.
    feed: Option<Arc<NoteFeed>>,
    missed: EventCursor,
    /// Counts the loads followed, so a reader tells one piece's notes from
    /// the next one's.
    run: u64,
}

impl RenderEngine {
    /// Follows the notes the piece plays from here on, and those of every
    /// piece loaded after: read them with [`Self::take_notes`] or
    /// [`Self::take_note_words`]. Rendering sounds the same either way.
    pub fn follow_notes(&mut self) {
        if self.notes.is_some() {
            return;
        }
        self.notes = Some(Followed::default());
        if let Some(graph) = self.graph.clone() {
            self.tap_notes(&mut graph.lock().unwrap());
        }
    }

    /// Taps `graph`, just installed, with a fresh feed, as a new run.
    pub(super) fn tap_notes(&mut self, graph: &mut SignalGraph) {
        let Some(followed) = self.notes.as_mut() else {
            return;
        };
        let (feed, out) = NoteFeed::new();
        let mirror = TopologyMirror::of(&graph.modules, &graph.edges);
        graph.note_taps = feed.keys().taps(&mirror);
        graph.note_out = Some(out);
        graph.note_keys = Some(Arc::clone(feed.keys()));
        followed.feed = Some(feed);
        followed.missed = EventCursor::new();
        followed.run += 1;
    }

    /// The notes played since the last read, paired, as
    /// [`crate::RunningInvention::take_notes`] gives them; nothing when
    /// not following. One reader: it and [`Self::take_note_words`] drain
    /// the same feed.
    pub fn take_notes(&self) -> NoteChanges {
        match self
            .notes
            .as_ref()
            .and_then(|followed| followed.feed.as_ref())
        {
            Some(feed) => feed.take(),
            None => NoteChanges::default(),
        }
    }

    /// Drains the notes played since the last read into `out`, unpaired, as
    /// words a [`notes::PlayedNotes`] takes (`out` of
    /// [`notes::NOTE_WORDS_LEN`] holds any drain). Returns the words
    /// written: 0 when not following, or with nothing loaded. Neither
    /// allocates nor frees, so a player may call it from its audio thread
    /// between renders.
    pub fn take_note_words(&mut self, out: &mut [f64]) -> usize {
        let (Some(followed), Some(graph)) = (self.notes.as_mut(), self.graph.as_ref()) else {
            return 0;
        };
        let Some(feed) = followed.feed.as_ref() else {
            return 0;
        };
        let graph = graph.lock().unwrap();
        notes::write_words(
            feed,
            &graph.note_taps,
            followed.run,
            &mut followed.missed,
            out,
        )
    }

    /// The module and clock ids the feed has given keys, by key: what
    /// [`notes::PlayedNotes::set_names`] takes. Allocates; ask again only
    /// when a drain counts more keys than were named.
    pub fn note_names(&self) -> Vec<String> {
        match self
            .notes
            .as_ref()
            .and_then(|followed| followed.feed.as_ref())
        {
            Some(feed) => feed.keys().names(),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
#[path = "notes_tests.rs"]
mod tests;
