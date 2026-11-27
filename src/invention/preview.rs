//! The notes a document will play over a range of bars, listed without
//! playing it: a private copy of the piece is built under a fixed seed and
//! run offline, its note taps drained as it goes (see `notes`), and the
//! notes that start in the range come back in pages.
//!
//! The run is bounded before it starts ([`MAX_BARS`] bars, and
//! [`MAX_SECONDS`] seconds of audio counted from the start of the piece to a
//! bar past the range) and can be advanced a slice at a time
//! ([`NotePreview::advance`]), so a host with one thread (a browser page)
//! can yield between slices. Nothing is shared with any other runtime: the
//! copy has its own graph and backend, starts no script host or agent
//! worker, and is dropped when the answer is taken.

use std::collections::HashMap;

use super::notes::NoteChange;
use super::{Invention, InventionBuilder, RunningInvention};
use crate::ControlValue;

mod answer;
mod backend;
mod request;
mod walk;

pub use request::{DEFAULT_BARS, MAX_BARS};

use answer::{Outcome, Range};
use backend::{Pull, PullBackend};
use request::Request;

/// The most audio one preview works through, in seconds at the preview's
/// sample rate.
pub const MAX_SECONDS: u64 = 300;

/// The sample rate the note tools preview at, whatever the host plays at,
/// so every host lists the same notes at the same seconds for a document.
pub const PREVIEW_SAMPLE_RATE: u32 = 48_000;

/// Seconds in a bar for a piece with no clock.
const CLOCKLESS_BAR_SECONDS: f64 = 2.0;

/// A clock reporting no meter counts bars of this many beats.
const DEFAULT_BEATS_PER_BAR: f64 = 4.0;

/// Frames rendered between two looks at the clock and the note taps: well
/// inside the note ring's room, and fine enough to find a bar line between
/// two looks when the tempo moves.
const LOOK_FRAMES: u64 = 256;

/// Beats a note's start may fall short of a bar line and still count as on
/// it: the clock counts beats in `f32`.
const BEAT_SLACK: f64 = 1e-4;

/// One preview in progress (see the module docs).
pub struct NotePreview {
    request: Request,
    fingerprint: u64,
    sample_rate: u32,
    running: Option<RunningInvention>,
    pull: Pull,
    span: Span,
    max_frames: u64,
    frame: u64,
    /// Where the main clock was at the last look: frame and beats.
    last: (u64, f64),
    /// Whether the main clock kept its starting tempo throughout.
    steady: bool,
    /// The frames at which the main clock crossed the range's first and
    /// last bar lines, found between two looks.
    crossed: [Option<f64>; 2],
    collected: Vec<NoteChange>,
    held: HashMap<(String, u64), usize>,
    walk: walk::Walk,
    /// Note starts and ends the taps dropped, their ring full.
    missed: u32,
    finished: bool,
    limit_reached: bool,
}

/// The range, on the main clock or, without one, in frames.
enum Span {
    Clock {
        id: String,
        bpm: f64,
        beats_per_bar: f64,
        /// The range's first and last bar lines, then the beat the run stops
        /// at: a bar past the range, to finish the notes still sounding.
        beats: [f64; 3],
    },
    Frames {
        frames: [u64; 3],
    },
}

impl NotePreview {
    /// Builds a private copy of `document` at `sample_rate` Hz and readies it
    /// to run for `request` (JSON; see `docs/preview-notes.md`). Refuses,
    /// before any audio is rendered, a malformed request, a document the
    /// engine cannot load (with the error loading it gives), an unknown
    /// module, and a range past the limits.
    pub fn new(document: &str, request: &str, sample_rate: u32) -> Result<Self, String> {
        // Read exactly as `RenderEngine::load_json` reads it, so a document
        // it refuses is refused here with the same words.
        let invention =
            serde_json::from_str::<Invention>(document).map_err(|error| error.to_string())?;
        Self::starting(invention, document, request, sample_rate)
    }

    /// As [`NotePreview::new`], for a document already read: one loaded
    /// from a file keeps its `source_path`, so the assets and developments
    /// it names by relative path are found as they are when it plays.
    pub fn for_invention(
        invention: Invention,
        request: &str,
        sample_rate: u32,
    ) -> Result<Self, String> {
        let document = serde_json::to_string(&invention).map_err(|error| error.to_string())?;
        Self::starting(invention, &document, request, sample_rate)
    }

    /// Builds the copy of `invention`, whose text `document` the request's
    /// cursor is checked against.
    fn starting(
        invention: Invention,
        document: &str,
        request: &str,
        sample_rate: u32,
    ) -> Result<Self, String> {
        let request = Request::parse(request)?;
        let fingerprint = request.fingerprint(document, sample_rate);
        if let Some(cursor) = &request.cursor {
            if !cursor.fits(fingerprint) {
                return Err(
                    "cursor belongs to a different request: pass the same document, \
                            bars, start_bar, module_id and seed as the call that gave it"
                        .to_string(),
                );
            }
        }
        if let Some(id) = &request.module_id {
            if !invention.modules.iter().any(|module| &module.id == id) {
                return Err(format!("module_id '{id}' is not a module of the invention"));
            }
        }
        let main_clock = invention.summary_clocks().first().map(|id| id.to_string());
        let walk = walk::walk(&invention);
        let (runtime, _handles) = InventionBuilder::new(sample_rate)
            .with_seed(request.seed)
            .build(invention)
            .map_err(|error| error.to_string())?;
        let (backend, pull) = PullBackend::new(sample_rate);
        let running = runtime
            .start_without_hosts(backend)
            .map_err(|error| error.to_string())?;

        let span = span(&running, main_clock, &request);
        if let Span::Clock { id, bpm, .. } = &span {
            if bpm.is_nan() || *bpm <= 0.0 {
                running.stop();
                return Err(format!(
                    "the main clock '{id}' starts at {bpm} bpm, so its bars never end"
                ));
            }
        }
        let max_frames = MAX_SECONDS * u64::from(sample_rate);
        let needed = span.frames_needed(sample_rate);
        if needed > max_frames as f64 {
            let needed = needed.ceil() as u64;
            running.stop();
            return Err(format!(
                "the range needs {needed} frames ({} seconds at {sample_rate} Hz); \
                 the largest allowed is {max_frames} frames ({MAX_SECONDS} seconds)",
                seconds_text(needed, sample_rate)
            ));
        }
        Ok(Self {
            request,
            fingerprint,
            sample_rate,
            running: Some(running),
            pull,
            span,
            max_frames,
            frame: 0,
            last: (0, 0.0),
            steady: true,
            crossed: [None, None],
            collected: Vec::new(),
            held: HashMap::new(),
            walk,
            missed: 0,
            finished: false,
            limit_reached: false,
        })
    }

    /// Runs the copy for up to `frames` more frames; true once the run is
    /// over (a bar past the range, or the frame limit).
    pub fn advance(&mut self, frames: u64) -> bool {
        let mut left = frames;
        while !self.finished && left > 0 {
            let step = LOOK_FRAMES.min(left).min(self.max_frames - self.frame);
            let mut done = 0;
            while done < step {
                let n = (step - done).min(crate::MAX_BLOCK as u64);
                self.pull.render(n as usize);
                done += n;
            }
            self.frame += step;
            left -= step;
            self.look();
        }
        self.finished
    }

    /// Runs the copy to its end and gives the answer.
    pub fn run(document: &str, request: &str, sample_rate: u32) -> Result<String, String> {
        let mut preview = Self::new(document, request, sample_rate)?;
        while !preview.advance(u64::from(sample_rate)) {}
        preview.answer()
    }

    /// The answer, as JSON: the page of notes the request's cursor points
    /// at. Runs the rest of the way first if the run is not over.
    pub fn answer(mut self) -> Result<String, String> {
        while !self.advance(u64::from(self.sample_rate)) {}
        self.stop();
        if self.missed > 0 {
            return Err(format!(
                "the piece starts and ends notes faster than the preview can follow: \
                 {} note starts and ends were lost, so the notes cannot be listed",
                self.missed
            ));
        }
        let range = self.range();
        let notes = std::mem::take(&mut self.collected)
            .into_iter()
            .filter(|note| range.holds(note, BEAT_SLACK))
            .collect();
        let outcome = Outcome {
            notes,
            range,
            seed: self.request.seed,
            walk: std::mem::take(&mut self.walk),
            limit_reached: self.limit_reached,
            max_frames: self.max_frames,
            sample_rate: self.sample_rate,
        };
        let offset = self.request.cursor.map_or(0, |cursor| cursor.offset);
        outcome.page(offset, self.fingerprint)
    }

    /// Drains the note taps and checks the clock after a stretch of frames.
    fn look(&mut self) {
        let Some(changes) = self.running.as_ref().map(|running| running.take_notes()) else {
            return;
        };
        self.missed += changes.missed;
        for change in changes.notes {
            self.collect(change);
        }
        let Some(running) = &self.running else {
            return;
        };
        let at_limit = self.frame >= self.max_frames;
        match &self.span {
            Span::Clock { id, bpm, beats, .. } => {
                let elapsed = number(running, id, "elapsed").unwrap_or(self.last.1);
                if number(running, id, "bpm").is_some_and(|now| now != *bpm) {
                    self.steady = false;
                }
                let (from_frame, from_beats) = self.last;
                for (crossed, line) in self.crossed.iter_mut().zip(beats) {
                    if crossed.is_none() && elapsed >= *line && *line >= from_beats {
                        let part = if elapsed > from_beats {
                            (line - from_beats) / (elapsed - from_beats)
                        } else {
                            0.0
                        };
                        *crossed =
                            Some(from_frame as f64 + part * (self.frame - from_frame) as f64);
                    }
                }
                self.last = (self.frame, elapsed);
                if elapsed >= beats[2] {
                    self.finished = true;
                } else if at_limit {
                    self.finished = true;
                    // A clock a hair short of the line at the very frame the
                    // limit allows has still played the range.
                    self.limit_reached = elapsed < beats[2] - 0.01;
                }
            }
            Span::Frames { frames } => {
                if self.frame >= frames[2] {
                    self.finished = true;
                } else if at_limit {
                    self.finished = true;
                    self.limit_reached = true;
                }
            }
        }
    }

    fn collect(&mut self, change: NoteChange) {
        if let Some(only) = &self.request.module_id {
            if &change.module_id != only {
                return;
            }
        }
        let key = (change.module_id.clone(), change.note);
        match self.held.get(&key) {
            Some(&index) => self.collected[index].end = change.end,
            None => {
                self.held.insert(key, self.collected.len());
                self.collected.push(change);
            }
        }
    }

    /// Where the range lies, in frames and, with a clock, in beats.
    fn range(&self) -> Range {
        let sample_rate = f64::from(self.sample_rate);
        match &self.span {
            Span::Clock {
                id,
                bpm,
                beats_per_bar,
                beats,
            } => {
                let frame_at = |index: usize| {
                    if self.steady {
                        beats[index] * 60.0 / bpm * sample_rate
                    } else {
                        self.crossed[index].unwrap_or(self.frame as f64)
                    }
                };
                Range {
                    start_bar: self.request.start_bar,
                    bars: self.request.bars,
                    clock: Some(id.clone()),
                    beats_per_bar: *beats_per_bar,
                    beats: Some([beats[0], beats[1]]),
                    frames: [frame_at(0), frame_at(1)],
                }
            }
            Span::Frames { frames } => Range {
                start_bar: self.request.start_bar,
                bars: self.request.bars,
                clock: None,
                beats_per_bar: DEFAULT_BEATS_PER_BAR,
                beats: None,
                frames: [frames[0] as f64, frames[1] as f64],
            },
        }
    }

    fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            running.stop();
        }
    }
}

impl Drop for NotePreview {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Span {
    /// The frames the run needs at the starting tempo.
    fn frames_needed(&self, sample_rate: u32) -> f64 {
        match self {
            Span::Clock { bpm, beats, .. } if *bpm > 0.0 => {
                // Exact for whole tempos; never rounds a whole count up.
                (beats[2] * 60.0 * f64::from(sample_rate) / bpm - 1e-6).max(0.0)
            }
            Span::Clock { .. } => f64::INFINITY,
            Span::Frames { frames } => frames[2] as f64,
        }
    }
}

/// The range on the main clock, at its starting tempo and meter, or in
/// 2-second bars without one.
fn span(running: &RunningInvention, main_clock: Option<String>, request: &Request) -> Span {
    let bars = [
        request.start_bar,
        request.start_bar.saturating_add(request.bars),
        request.start_bar.saturating_add(request.bars + 1),
    ];
    let clock = main_clock.and_then(|id| Some((number(running, &id, "bpm")?, id)));
    match clock {
        Some((bpm, id)) => {
            let beats_per_bar = number(running, &id, "beats_per_bar")
                .filter(|beats| *beats > 0.0)
                .unwrap_or(DEFAULT_BEATS_PER_BAR);
            Span::Clock {
                id,
                bpm,
                beats_per_bar,
                beats: bars.map(|bar| bar as f64 * beats_per_bar),
            }
        }
        None => {
            let bar_frames = CLOCKLESS_BAR_SECONDS * f64::from(running.sample_rate);
            Span::Frames {
                frames: bars.map(|bar| (bar as f64 * bar_frames).round() as u64),
            }
        }
    }
}

fn number(running: &RunningInvention, module_id: &str, key: &str) -> Option<f64> {
    match running.get_control(module_id, key).ok()? {
        ControlValue::Number(value) => Some(f64::from(value)),
        _ => None,
    }
}

/// `frames` as seconds, whole when it is.
fn seconds_text(frames: u64, sample_rate: u32) -> String {
    let seconds = frames as f64 / f64::from(sample_rate);
    if seconds.fract() == 0.0 {
        format!("{seconds:.0}")
    } else {
        format!("{seconds:.2}")
    }
}

#[cfg(test)]
mod tests;
