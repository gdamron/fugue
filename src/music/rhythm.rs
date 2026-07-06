//! Exact rhythmic time and note-value naming.
//!
//! [`Rat`] measures musical time as a reduced fraction of a quarter note, so
//! onsets, durations, and grids stay exact — no float rounding can move a
//! note off its true position. [`note_value_name`] names a duration in
//! notation terms (`quarter_note`, `16th_note`, `8th_triplet`, …), the
//! vocabulary used by `rhythm_grid` in `fugue.score.v1` assets.

/// An exact time value in quarter notes. Always reduced, denominator >= 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rat {
    num: i64,
    den: i64,
}

impl Rat {
    /// Creates a reduced rational. `den` must be positive.
    pub fn new(num: i64, den: i64) -> Self {
        assert!(den > 0, "Rat denominator must be positive");
        let g = gcd_i64(num.unsigned_abs(), den.unsigned_abs()).max(1) as i64;
        Self {
            num: num / g,
            den: den / g,
        }
    }

    /// Numerator of the reduced fraction.
    pub fn num(self) -> i64 {
        self.num
    }

    /// Denominator of the reduced fraction (always positive).
    pub fn den(self) -> i64 {
        self.den
    }

    /// GCD of two rationals: gcd(a/b, c/d) = gcd(ad, cb) / bd.
    ///
    /// The GCD of a set of onsets/durations is the coarsest grid every value
    /// sits on exactly — how importers infer a piece's rhythm grid.
    pub fn gcd(self, other: Rat) -> Rat {
        Rat::new(
            gcd_i64(
                (self.num * other.den).unsigned_abs(),
                (other.num * self.den).unsigned_abs(),
            ) as i64,
            self.den * other.den,
        )
    }

    /// `self / grid` when it is an exact non-negative integer — the step
    /// index (or step count) of this time value on a rhythm grid.
    pub fn div_exact(self, grid: Rat) -> Option<i64> {
        let num = self.num * grid.den;
        let den = self.den * grid.num;
        if den != 0 && num % den == 0 && num / den >= 0 {
            Some(num / den)
        } else {
            None
        }
    }

    pub fn is_zero(self) -> bool {
        self.num == 0
    }

    pub fn is_negative(self) -> bool {
        self.num < 0
    }

    pub fn is_positive(self) -> bool {
        self.num > 0
    }
}

impl std::ops::Add for Rat {
    type Output = Rat;

    fn add(self, other: Rat) -> Rat {
        Rat::new(
            self.num * other.den + other.num * self.den,
            self.den * other.den,
        )
    }
}

impl std::ops::Sub for Rat {
    type Output = Rat;

    fn sub(self, other: Rat) -> Rat {
        Rat::new(
            self.num * other.den - other.num * self.den,
            self.den * other.den,
        )
    }
}

impl PartialOrd for Rat {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Rat {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.num * other.den).cmp(&(other.num * self.den))
    }
}

fn gcd_i64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Names a duration (in quarter notes) as a note value, e.g. `16th_note` or
/// `8th_triplet` — the vocabulary `fugue.score.v1` uses for `rhythm_grid`
/// (cf. `examples/in_c/score.json`'s `32nd_note`).
///
/// Durations that are not a unit fraction of a whole note fall back to
/// `{num}/{den}_whole_note`.
pub fn note_value_name(quarters: Rat) -> String {
    let whole = Rat::new(quarters.num, quarters.den * 4);
    if whole.num == 1 {
        match whole.den {
            1 => return "whole_note".to_string(),
            2 => return "half_note".to_string(),
            4 => return "quarter_note".to_string(),
            8 => return "8th_note".to_string(),
            16 => return "16th_note".to_string(),
            32 => return "32nd_note".to_string(),
            64 => return "64th_note".to_string(),
            128 => return "128th_note".to_string(),
            3 => return "half_triplet".to_string(),
            6 => return "quarter_triplet".to_string(),
            12 => return "8th_triplet".to_string(),
            24 => return "16th_triplet".to_string(),
            48 => return "32nd_triplet".to_string(),
            _ => {}
        }
    }
    format!("{}/{}_whole_note", whole.num, whole.den)
}

/// Inverse of [`note_value_name`]: parses a note-value name back to a
/// duration in quarter notes. Accepts every name `note_value_name` produces,
/// including the `{num}/{den}_whole_note` fallback.
pub fn note_value_from_name(name: &str) -> Option<Rat> {
    let whole = match name {
        "whole_note" => Rat::new(1, 1),
        "half_note" => Rat::new(1, 2),
        "quarter_note" => Rat::new(1, 4),
        "8th_note" => Rat::new(1, 8),
        "16th_note" => Rat::new(1, 16),
        "32nd_note" => Rat::new(1, 32),
        "64th_note" => Rat::new(1, 64),
        "128th_note" => Rat::new(1, 128),
        "half_triplet" => Rat::new(1, 3),
        "quarter_triplet" => Rat::new(1, 6),
        "8th_triplet" => Rat::new(1, 12),
        "16th_triplet" => Rat::new(1, 24),
        "32nd_triplet" => Rat::new(1, 48),
        other => {
            let fraction = other.strip_suffix("_whole_note")?;
            let (num, den) = fraction.split_once('/')?;
            let num = num.parse::<i64>().ok().filter(|&n| n > 0)?;
            let den = den.parse::<i64>().ok().filter(|&d| d > 0)?;
            Rat::new(num, den)
        }
    };
    Some(Rat::new(whole.num * 4, whole.den))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduces_and_compares_exactly() {
        assert_eq!(Rat::new(4, 8), Rat::new(1, 2));
        assert!(Rat::new(1, 3) < Rat::new(1, 2));
        assert_eq!(Rat::new(1, 4) + Rat::new(1, 4), Rat::new(1, 2));
        assert_eq!(Rat::new(1, 2) - Rat::new(1, 4), Rat::new(1, 4));
    }

    #[test]
    fn gcd_finds_the_coarsest_common_grid() {
        // A dotted eighth (3/4 quarter) and a sixteenth (1/4) share a 16th grid.
        assert_eq!(Rat::new(3, 4).gcd(Rat::new(1, 4)), Rat::new(1, 4));
        assert_eq!(Rat::new(4, 1).gcd(Rat::new(1, 2)), Rat::new(1, 2));
    }

    #[test]
    fn div_exact_yields_grid_steps_only_when_exact() {
        assert_eq!(Rat::new(3, 2).div_exact(Rat::new(1, 2)), Some(3));
        assert_eq!(Rat::new(1, 3).div_exact(Rat::new(1, 2)), None);
        assert_eq!(Rat::new(-1, 2).div_exact(Rat::new(1, 2)), None);
    }

    #[test]
    fn note_value_names_round_trip() {
        for quarters in [
            Rat::new(4, 1),
            Rat::new(1, 1),
            Rat::new(1, 4),
            Rat::new(1, 8),
            Rat::new(1, 3),
            Rat::new(2, 3),
            Rat::new(3, 4), // dotted eighth → fallback name
        ] {
            let name = note_value_name(quarters);
            assert_eq!(
                note_value_from_name(&name),
                Some(quarters),
                "round trip failed for {}",
                name
            );
        }
        assert_eq!(note_value_from_name("not_a_value"), None);
        assert_eq!(note_value_from_name("0/4_whole_note"), None);
    }

    #[test]
    fn names_binary_and_triplet_note_values() {
        assert_eq!(note_value_name(Rat::new(4, 1)), "whole_note");
        assert_eq!(note_value_name(Rat::new(1, 1)), "quarter_note");
        assert_eq!(note_value_name(Rat::new(1, 4)), "16th_note");
        assert_eq!(note_value_name(Rat::new(1, 8)), "32nd_note");
        assert_eq!(note_value_name(Rat::new(1, 3)), "8th_triplet");
        assert_eq!(note_value_name(Rat::new(2, 3)), "quarter_triplet");
        assert_eq!(note_value_name(Rat::new(3, 4)), "3/16_whole_note");
    }
}
