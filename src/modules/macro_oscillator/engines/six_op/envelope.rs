//! An operator's envelope: a quick rise, two decays, a sustain and a
//! release, all in our own units (seconds and decibels).
//!
//! The rise is a straight line in amplitude, so a note fades in rather than
//! clicking even from silence. The decays and the release move in
//! decibels at a steady rate, the way a struck or plucked sound dies away;
//! each is given as the time to fall 60 dB. Every time can be stretched,
//! which is how `morph` and the keyboard's rate scaling lengthen or shorten
//! a note.
//!
//! The envelope can also be read at a point in time, as if the key had been
//! held that long: the drone with nothing patched to `trigger` scrubs
//! through it.

/// The level below which an envelope is silent, in dB.
pub(in crate::modules::macro_oscillator) const FLOOR_DB: f32 = -100.0;

/// The shortest rise, in seconds, so even an instant attack is a ramp.
const MIN_ATTACK: f32 = 0.001;

/// The shortest decay or release, as a time to fall 60 dB.
const MIN_T60: f32 = 0.005;

/// An envelope's shape, as authored in a patch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::modules::macro_oscillator) struct Shape {
    /// The time to rise from silence to full level, in seconds.
    pub attack: f32,
    /// The first decay: its rate (seconds to fall 60 dB) and the level it
    /// heads for, in dB below full.
    pub decay: f32,
    pub break_db: f32,
    /// The second decay: its rate, and the level it settles at while the
    /// key is held (at or below [`FLOOR_DB`] for a sound that dies away).
    pub second_decay: f32,
    pub sustain_db: f32,
    /// The release once the key is let go: seconds to fall 60 dB.
    pub release: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    SecondDecay,
    Sustain,
    Release,
}

/// One running envelope.
#[derive(Clone, Copy, Debug)]
pub(in crate::modules::macro_oscillator) struct Envelope {
    stage: Stage,
    /// The stage it was in when the key was let go: a release never falls
    /// slower than the decay it interrupts.
    released_from: Stage,
    /// The level, in dB (`FLOOR_DB` and below is silent).
    db: f32,
    /// The level as a gain, kept in step with `db`.
    gain: f32,
}

impl Envelope {
    pub fn new() -> Self {
        Self {
            stage: Stage::Idle,
            released_from: Stage::Idle,
            db: FLOOR_DB,
            gain: 0.0,
        }
    }

    /// The level as a gain, 0 to 1.
    #[cfg(test)]
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// The level in dB ([`FLOOR_DB`] when silent).
    pub fn db(&self) -> f32 {
        self.db
    }

    /// Whether it has died away.
    pub fn is_idle(&self) -> bool {
        self.stage == Stage::Idle
    }

    /// Whether it is still rising.
    pub fn is_attacking(&self) -> bool {
        self.stage == Stage::Attack
    }

    /// Starts a note: rises from wherever it is now.
    pub fn start(&mut self) {
        self.stage = Stage::Attack;
    }

    /// Lets the key go.
    pub fn release(&mut self) {
        if self.stage != Stage::Idle && self.stage != Stage::Release {
            self.released_from = self.stage;
            self.stage = Stage::Release;
        }
    }

    /// Silences it at once, for a voice that is not sounding.
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// Moves `seconds` on, with every time in `shape` multiplied by
    /// `stretch`, and returns the new gain.
    pub fn advance(&mut self, shape: &Shape, seconds: f32, stretch: f32) -> f32 {
        // The shortest times hold after stretching, not before.
        let stretch = stretch.max(1e-3);
        match self.stage {
            Stage::Idle => return 0.0,
            Stage::Attack => {
                self.gain += seconds / (shape.attack * stretch).max(MIN_ATTACK);
                if self.gain >= 1.0 - 1e-6 {
                    self.gain = 1.0;
                    self.db = 0.0;
                    self.stage = Stage::Decay;
                } else {
                    self.db = to_db(self.gain);
                }
                return self.gain;
            }
            Stage::Decay => {
                if approach(&mut self.db, shape.break_db, shape.decay * stretch, seconds) {
                    self.stage = Stage::SecondDecay;
                }
            }
            Stage::SecondDecay => {
                if approach(
                    &mut self.db,
                    shape.sustain_db,
                    shape.second_decay * stretch,
                    seconds,
                ) {
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => {}
            Stage::Release => {
                let falling = match self.released_from {
                    Stage::Decay if shape.break_db < self.db => shape.decay,
                    Stage::SecondDecay if shape.sustain_db < self.db => shape.second_decay,
                    _ => f32::INFINITY,
                };
                approach(
                    &mut self.db,
                    FLOOR_DB,
                    shape.release.min(falling) * stretch,
                    seconds,
                );
            }
        }
        if self.db <= FLOOR_DB {
            self.clear();
        } else {
            self.gain = to_gain(self.db);
        }
        self.gain
    }

    /// Heads toward `target_db` at no more than `db_per_second`, holding a
    /// drone's level where the scrub puts it without jumping there.
    pub fn track(&mut self, target_db: f32, seconds: f32, db_per_second: f32) -> f32 {
        let target = target_db.max(FLOOR_DB);
        let reach = db_per_second * seconds;
        self.db = if self.db < target {
            (self.db + reach).min(target)
        } else {
            (self.db - reach).max(target)
        };
        self.stage = Stage::Sustain;
        if self.db <= FLOOR_DB {
            self.clear();
        } else {
            self.gain = to_gain(self.db);
        }
        self.gain
    }

    /// Jumps to `db` and holds there, for a drone's first block, so it
    /// starts where the scrub points.
    pub fn set(&mut self, db: f32) {
        self.db = FLOOR_DB;
        self.track(db, 1.0, f32::INFINITY);
    }
}

/// The level `seconds` after the peak of the rise, with the key held, in
/// dB, with every time in `shape` multiplied by `stretch`.
pub(in crate::modules::macro_oscillator) fn held_level(
    shape: &Shape,
    seconds: f32,
    stretch: f32,
) -> f32 {
    let stretch = stretch.max(1e-3);
    let mut t = seconds.max(0.0);
    let mut db = 0.0;
    for (target, t60) in [
        (shape.break_db, shape.decay),
        (shape.sustain_db, shape.second_decay),
    ] {
        let rate = 60.0 / (t60 * stretch).max(MIN_T60);
        let span = (target - db).abs() / rate;
        if t <= span {
            return db + (target - db).signum() * rate * t;
        }
        t -= span;
        db = target;
    }
    db.max(FLOOR_DB)
}

/// Moves `db` toward `target` at 60 dB per `t60` seconds for `dt`
/// seconds; whether it got there.
fn approach(db: &mut f32, target: f32, t60: f32, dt: f32) -> bool {
    let reach = 60.0 / t60.max(MIN_T60) * dt;
    if (*db - target).abs() <= reach {
        *db = target;
        true
    } else {
        *db += (target - *db).signum() * reach;
        false
    }
}

#[inline]
fn to_gain(db: f32) -> f32 {
    (db * (std::f32::consts::LOG2_10 / 20.0)).exp2()
}

#[inline]
fn to_db(gain: f32) -> f32 {
    if gain <= 0.0 {
        FLOOR_DB
    } else {
        (20.0 * gain.log10()).max(FLOOR_DB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: f32 = 12.0 / 48_000.0;

    fn bell() -> Shape {
        Shape {
            attack: 0.002,
            decay: 0.5,
            break_db: -20.0,
            second_decay: 4.0,
            sustain_db: FLOOR_DB,
            release: 1.0,
        }
    }

    /// The gain after `seconds` of `shape`, the key held throughout.
    fn after(shape: &Shape, seconds: f32, stretch: f32) -> f32 {
        let mut envelope = Envelope::new();
        envelope.start();
        let mut gain = 0.0;
        for _ in 0..(seconds / BLOCK).round() as usize {
            gain = envelope.advance(shape, BLOCK, stretch);
        }
        gain
    }

    #[test]
    fn it_rises_in_a_line_then_decays_in_decibels() {
        let shape = bell();
        assert!((after(&shape, 0.001, 1.0) - 0.5).abs() < 0.01);
        assert_eq!(after(&shape, 0.002, 1.0), 1.0);
        // 0.25 s after the peak, the first decay has fallen 30 dB... but it
        // stops at the break (-20 dB) after 1/6 s and goes on at 60 dB per
        // 4 s: 20 + (0.25 - 1/6) * 15 = 21.25 dB down.
        let db = 20.0 * after(&shape, 0.252, 1.0).log10();
        assert!((db + 21.25).abs() < 0.1, "{db}");
    }

    #[test]
    fn reading_the_held_level_agrees_with_running_it() {
        let shape = bell();
        for seconds in [0.05, 0.1, 0.5, 1.0, 2.0] {
            for stretch in [0.25, 1.0, 4.0] {
                let run = to_db(after(&shape, 0.002 * stretch + seconds, stretch));
                let read = held_level(&shape, seconds, stretch);
                assert!(
                    (run - read).abs() < 0.2,
                    "{seconds} s at {stretch}x: {run} {read}"
                );
            }
        }
    }

    #[test]
    fn stretching_lengthens_every_stage() {
        let shape = bell();
        assert!(after(&shape, 1.0, 2.0) > after(&shape, 1.0, 1.0));
        assert!(after(&shape, 1.0, 0.5) < after(&shape, 1.0, 1.0));
    }

    #[test]
    fn the_shortest_rise_holds_under_any_stretch() {
        let shape = Shape {
            attack: 0.001,
            ..bell()
        };
        let mut envelope = Envelope::new();
        envelope.start();
        envelope.advance(&shape, 0.0005, 0.125);
        assert!(envelope.is_attacking());
        assert!((envelope.gain() - 0.5).abs() < 1e-3, "{}", envelope.gain());
    }

    #[test]
    fn release_dies_away_to_idle_and_a_restart_rises_from_where_it_is() {
        let shape = Shape {
            sustain_db: -6.0,
            ..bell()
        };
        let mut envelope = Envelope::new();
        envelope.start();
        for _ in 0..12_000 {
            envelope.advance(&shape, BLOCK, 1.0);
        }
        let held = envelope.gain();
        assert!((20.0 * held.log10() + 6.0).abs() < 0.01);
        envelope.release();
        // 100 dB at 60 dB per second.
        for _ in 0..((100.0 / 60.0 / BLOCK) as usize + 2) {
            envelope.advance(&shape, BLOCK, 1.0);
        }
        assert!(envelope.is_idle());
        assert_eq!(envelope.gain(), 0.0);

        // From silence the rise starts at 0; from -6 dB it carries on up.
        envelope.start();
        let from_silence = envelope.advance(&shape, BLOCK, 1.0);
        let mut sounding = Envelope::new();
        sounding.set(-6.0);
        sounding.start();
        let from_half = sounding.advance(&shape, BLOCK, 1.0);
        assert!((from_half - (to_gain(-6.0) + from_silence)).abs() < 1e-4);
    }

    #[test]
    fn tracking_moves_at_a_bounded_rate() {
        let mut envelope = Envelope::new();
        envelope.set(0.0);
        let gain = envelope.track(-40.0, 0.01, 600.0);
        assert!((20.0 * gain.log10() + 6.0).abs() < 1e-3);
    }
}
