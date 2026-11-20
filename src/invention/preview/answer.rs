//! A preview's answer: the notes that start in the range, in order, and the
//! page of them a request's cursor points at.

use serde::Serialize;

use super::request::{Cursor, MAX_BARS};
use super::walk::Walk;
use crate::invention::notes::{NoteChange, NoteTime};

/// The most notes one answer holds.
pub(super) const PAGE_NOTES: usize = 256;

/// The most bytes of JSON one answer holds.
pub(super) const PAGE_BYTES: usize = 48 * 1024;

/// A note the run played, as the note taps reported it.
pub(super) struct Collected {
    module_id: String,
    voice: u32,
    pitch: f32,
    velocity: f32,
    clock: Option<String>,
    start: NoteTime,
    pub(super) end: Option<NoteTime>,
}

impl Collected {
    /// The MIDI note, rounded, as the answer reports it.
    fn midi(&self) -> u8 {
        self.pitch.round().clamp(0.0, 127.0) as u8
    }
}

impl From<NoteChange> for Collected {
    fn from(change: NoteChange) -> Self {
        Self {
            module_id: change.module_id,
            voice: change.voice,
            pitch: change.pitch,
            velocity: change.velocity,
            clock: change.clock,
            start: change.start,
            end: change.end,
        }
    }
}

/// Where the range lies.
pub(super) struct Range {
    pub(super) start_bar: u64,
    pub(super) bars: u64,
    pub(super) clock: Option<String>,
    pub(super) beats_per_bar: f64,
    /// Its first and last bar lines on the main clock, when there is one.
    pub(super) beats: Option<[f64; 2]>,
    /// Its first and last bar lines in frames.
    pub(super) frames: [f64; 2],
}

impl Range {
    /// Whether `note` starts in the range: by beats when it counts on the
    /// main clock, by frames otherwise.
    pub(super) fn holds(&self, note: &Collected, slack: f64) -> bool {
        match (&self.beats, &note.clock, note.start.beats) {
            (Some([from, to]), Some(clock), Some(beats)) if Some(clock) == self.clock.as_ref() => {
                beats + slack >= *from && beats + slack < *to
            }
            _ => {
                let frame = note.start.frame as f64;
                frame >= self.frames[0] && frame < self.frames[1]
            }
        }
    }
}

/// Everything an answer is made from.
pub(super) struct Outcome {
    pub(super) notes: Vec<Collected>,
    pub(super) range: Range,
    pub(super) seed: u64,
    pub(super) walk: Walk,
    pub(super) limit_reached: bool,
    pub(super) max_frames: u64,
    pub(super) sample_rate: u32,
}

#[derive(Serialize)]
struct Answer<'a> {
    notes: &'a [NoteOut<'a>],
    range: RangeOut<'a>,
    seed: u64,
    chance: ChanceOut<'a>,
    not_run: Vec<NotRunOut<'a>>,
    truncated: bool,
    cursor: Option<String>,
    limit_reached: bool,
    limits: LimitsOut,
}

#[derive(Serialize)]
struct NoteOut<'a> {
    module_id: &'a str,
    voice: String,
    pitch: u8,
    velocity: f64,
    clock: Option<&'a str>,
    start_beats: Option<f64>,
    duration_beats: Option<f64>,
    start_seconds: f64,
    duration_seconds: Option<f64>,
}

#[derive(Serialize)]
struct RangeOut<'a> {
    start_bar: u64,
    bars: u64,
    clock: Option<&'a str>,
    beats_per_bar: f64,
    start_seconds: f64,
    end_seconds: f64,
}

#[derive(Serialize)]
struct ChanceOut<'a> {
    involved: bool,
    modules: &'a [String],
    message: String,
}

#[derive(Serialize)]
struct NotRunOut<'a> {
    module_id: &'a str,
    #[serde(rename = "type")]
    module_type: &'a str,
}

#[derive(Serialize)]
struct LimitsOut {
    max_bars: u64,
    max_frames: u64,
}

impl Outcome {
    /// The page starting at note `offset`, as JSON.
    pub(super) fn page(mut self, offset: usize, fingerprint: u64) -> Result<String, String> {
        let sample_rate = f64::from(self.sample_rate);
        // By start, then module id, then the pitch as reported, then voice.
        self.notes.sort_by(|a, b| {
            (a.start.frame, &a.module_id)
                .cmp(&(b.start.frame, &b.module_id))
                .then_with(|| a.midi().cmp(&b.midi()))
                .then_with(|| a.voice.cmp(&b.voice))
        });
        let total = self.notes.len();
        if offset > 0 && offset >= total {
            return Err(format!(
                "cursor points past the last note ({total} in all): it does not fit this request"
            ));
        }
        let notes: Vec<NoteOut> = self.notes[offset..]
            .iter()
            .map(|note| note_out(note, sample_rate))
            .collect();
        let chance = chance_out(&self.walk.chance);
        let not_run = self
            .walk
            .not_run
            .iter()
            .map(|found| NotRunOut {
                module_id: &found.module_id,
                module_type: &found.module_type,
            })
            .collect();
        let range = &self.range;
        let mut answer = Answer {
            notes: &[],
            range: RangeOut {
                start_bar: range.start_bar,
                bars: range.bars,
                clock: range.clock.as_deref(),
                beats_per_bar: range.beats_per_bar,
                start_seconds: round(range.frames[0] / sample_rate),
                end_seconds: round(range.frames[1] / sample_rate),
            },
            seed: self.seed,
            chance,
            not_run,
            // The largest the envelope gets: truncated, with the longest cursor.
            truncated: true,
            cursor: Some(Cursor::new(total, fingerprint).encode()),
            limit_reached: self.limit_reached,
            limits: LimitsOut {
                max_bars: MAX_BARS,
                max_frames: self.max_frames,
            },
        };
        let envelope = to_json(&answer)?.len();
        let mut bytes = envelope;
        let mut count = 0;
        for note in notes.iter().take(PAGE_NOTES) {
            let size = to_json(note)?.len() + usize::from(count > 0);
            if count > 0 && bytes + size > PAGE_BYTES {
                break;
            }
            bytes += size;
            count += 1;
        }
        let more = count < notes.len();
        answer.notes = &notes[..count];
        answer.truncated = more;
        answer.cursor = more.then(|| Cursor::new(offset + count, fingerprint).encode());
        to_json(&answer)
    }
}

fn note_out(note: &Collected, sample_rate: f64) -> NoteOut<'_> {
    let start_beats = note.start.beats;
    let end = note.end.as_ref();
    NoteOut {
        module_id: &note.module_id,
        voice: note.voice.to_string(),
        pitch: note.midi(),
        velocity: (f64::from(note.velocity.clamp(0.0, 1.0)) * 1e4).round() / 1e4,
        clock: note.clock.as_deref(),
        start_beats: start_beats.map(round),
        duration_beats: match (start_beats, end.and_then(|end| end.beats)) {
            (Some(start), Some(end)) => Some(round(end - start)),
            _ => None,
        },
        start_seconds: round(note.start.frame as f64 / sample_rate),
        duration_seconds: end
            .map(|end| round(end.frame.saturating_sub(note.start.frame) as f64 / sample_rate)),
    }
}

fn chance_out(modules: &[String]) -> ChanceOut<'_> {
    let message = if modules.is_empty() {
        "No module picks notes by chance: these are the notes the piece plays, whatever the seed."
            .to_string()
    } else {
        "These notes are one possible playing: some modules draw on chance, so a different \
         seed may give different notes where a module has no seed of its own."
            .to_string()
    };
    ChanceOut {
        involved: !modules.is_empty(),
        modules,
        message,
    }
}

/// Rounds to the microsecond (or millionth of a beat), which keeps pages
/// small and is far finer than a frame.
fn round(value: f64) -> f64 {
    (value * 1e6).round() / 1e6
}

fn to_json<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|error| error.to_string())
}
