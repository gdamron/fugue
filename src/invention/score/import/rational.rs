//! Exact rational time for score conversion.
//!
//! Onsets, durations, and measure lengths are tracked as reduced fractions of
//! a quarter note so grid inference and quantization are lossless — no float
//! rounding can move a note off its true position.

/// An exact time value in quarter notes. Always reduced, `den >= 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Rat {
    pub(super) num: i64,
    pub(super) den: i64,
}

impl Rat {
    pub(super) fn new(num: i64, den: i64) -> Self {
        debug_assert!(den > 0);
        let g = gcd_i64(num.unsigned_abs(), den.unsigned_abs()).max(1) as i64;
        Self {
            num: num / g,
            den: den / g,
        }
    }

    pub(super) fn add(self, other: Rat) -> Rat {
        Rat::new(
            self.num * other.den + other.num * self.den,
            self.den * other.den,
        )
    }

    pub(super) fn sub(self, other: Rat) -> Rat {
        Rat::new(
            self.num * other.den - other.num * self.den,
            self.den * other.den,
        )
    }

    /// GCD of two positive rationals: gcd(a/b, c/d) = gcd(ad, cb) / bd.
    pub(super) fn gcd(self, other: Rat) -> Rat {
        Rat::new(
            gcd_i64(
                (self.num * other.den).unsigned_abs(),
                (other.num * self.den).unsigned_abs(),
            ) as i64,
            self.den * other.den,
        )
    }

    /// `self / grid` when it is an exact non-negative integer.
    pub(super) fn div_exact(self, grid: Rat) -> Option<i64> {
        let num = self.num * grid.den;
        let den = self.den * grid.num;
        if den != 0 && num % den == 0 && num / den >= 0 {
            Some(num / den)
        } else {
            None
        }
    }

    pub(super) fn is_negative(self) -> bool {
        self.num < 0
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
