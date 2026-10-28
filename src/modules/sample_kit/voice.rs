//! A sample kit slot's one voice, and the short fade a retrigger gives the
//! sound it cuts off.

use crate::dsp::LinearRamp;
use crate::modules::sample_loading::SampleData;
use crate::payload::Shared;

/// How long a retriggered slot's old sound fades out under its new play:
/// long enough not to click, short enough to hear as the same hit.
pub(super) const DECLICK_SECONDS: f32 = 0.003;

/// Which of a voice's handles a fading tail reads.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Sample,
    Previous,
    Held,
}

/// The sound a retrigger cut off, fading out: its gain frozen at the cut,
/// so it never steps, read from one of the voice's handles so it holds no
/// handle of its own to free on the audio thread.
struct Tail {
    source: Source,
    position: usize,
    amp: f32,
    gain: f32,
}

/// One playback voice per slot. Retriggering a slot restarts it (per-slot
/// choke), fading out what it was playing; different slots mix freely.
pub(super) struct Voice {
    /// What the slot's next play plays.
    sample: Shared<SampleData>,
    /// The sample a swap replaced while it sounded, which rings out rather
    /// than stopping dead (a click). Held until a later swap retires it;
    /// the sample cache holds its buffer anyway.
    previous: Shared<SampleData>,
    /// A buffer kept for a fading tail when a swap would otherwise hand it
    /// back mid-fade; what it held before goes back instead.
    held: Shared<SampleData>,
    /// Whether the voice is playing `previous`.
    ringing_out: bool,
    /// Whether a `play` started the voice since it last rendered: it has
    /// sounded nothing yet, so a swap at the same instant reaches it.
    pub(super) fresh: bool,
    /// The slot's level, gliding to each write while the slot sounds, as a
    /// mixer's fader does: a level that stepped mid-sample would click.
    pub(super) level: LinearRamp,
    position: usize,
    pub(super) active: bool,
    tail: Tail,
    /// The tail's fade per frame.
    declick_step: f32,
}

impl Voice {
    pub(super) fn new(
        sample: Shared<SampleData>,
        level: f32,
        glide: u32,
        sample_rate: u32,
    ) -> Self {
        Self {
            previous: sample.clone(),
            held: sample.clone(),
            sample,
            ringing_out: false,
            fresh: false,
            level: LinearRamp::new(level, glide),
            position: 0,
            active: false,
            tail: Tail {
                source: Source::Sample,
                position: 0,
                amp: 0.0,
                gain: 0.0,
            },
            declick_step: 1.0 / (DECLICK_SECONDS * sample_rate as f32).max(1.0),
        }
    }

    /// Plays the slot from its start. A sound still playing fades out over
    /// [`DECLICK_SECONDS`] instead of stopping dead.
    pub(super) fn start(&mut self) {
        // A voice that has sounded nothing yet (played again at the
        // instant it started) has nothing to fade.
        if self.active && self.position > 0 {
            self.tail = Tail {
                source: if self.ringing_out {
                    Source::Previous
                } else {
                    Source::Sample
                },
                position: self.position,
                amp: self.level.value(),
                gain: 1.0,
            };
        }
        // A play restarts the slot's one voice, so it sounds at the level
        // written: a glide smooths only a sound that carries on.
        self.level.settle();
        self.ringing_out = false;
        self.position = 0;
        self.active = self.sample.len() > 0;
    }

    fn handle(&self, source: Source) -> &SampleData {
        match source {
            Source::Sample => &self.sample,
            Source::Previous => &self.previous,
            Source::Held => &self.held,
        }
    }

    fn sounding(&self) -> &SampleData {
        match self.ringing_out {
            true => &self.previous,
            false => &self.sample,
        }
    }

    /// Whether the voice adds anything this frame.
    #[inline]
    pub(super) fn audible(&self) -> bool {
        self.active || self.tail.gain > 0.0
    }

    /// The voice's next frame: its sound at its level, plus any fading
    /// tail.
    #[inline]
    pub(super) fn next_frame(&mut self) -> (f32, f32) {
        let (mut left, mut right) = self.fade_out();
        if self.active {
            let sample = self.sounding();
            let (l, r) = sample.frame_at(self.position);
            let len = sample.len();
            let level = self.level.next_value();
            left += l * level;
            right += r * level;
            self.position += 1;
            if self.position >= len {
                self.active = false;
                self.position = 0;
            }
        }
        (left, right)
    }

    #[inline]
    fn fade_out(&mut self) -> (f32, f32) {
        let tail = &self.tail;
        if tail.gain <= 0.0 {
            return (0.0, 0.0);
        }
        let sample = self.handle(tail.source);
        if tail.position >= sample.len() {
            self.tail.gain = 0.0;
            return (0.0, 0.0);
        }
        let (l, r) = sample.frame_at(tail.position);
        let amp = tail.amp * tail.gain;
        self.tail.position += 1;
        self.tail.gain = (self.tail.gain - self.declick_step).max(0.0);
        (l * amp, r * amp)
    }

    /// Takes `sample` for the next play, handing back the handle it no
    /// longer needs: the sounding one rings out to its end, and a fading
    /// tail keeps reading the buffer it was reading. A play at the same
    /// instant, which has sounded nothing yet, takes it instead, as it
    /// would had the swap been written first.
    pub(super) fn swap(&mut self, sample: Shared<SampleData>) -> Shared<SampleData> {
        let fading = (self.tail.gain > 0.0).then_some(self.tail.source);
        let replaced = std::mem::replace(&mut self.sample, sample);
        let (mut returned, from) = if self.fresh {
            self.active = self.sample.len() > 0;
            (replaced, Source::Sample)
        } else if !self.active || self.ringing_out {
            (replaced, Source::Sample)
        } else {
            self.ringing_out = true;
            if fading == Some(Source::Sample) {
                // The tail's buffer moves to `previous` with the sound.
                self.tail.source = Source::Previous;
            }
            (
                std::mem::replace(&mut self.previous, replaced),
                Source::Previous,
            )
        };
        if fading == Some(from) {
            std::mem::swap(&mut self.held, &mut returned);
            self.tail.source = Source::Held;
        }
        returned
    }
}
