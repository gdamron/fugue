//! Typed reads of the numbers in a module's config, so every module agrees
//! on what a number means. Read on the control thread, while a module is
//! built.
//!
//! - An integer key accepts a JSON integer, or a float that is exactly whole
//!   and in range (`72.0` reads as 72). A fractional or out-of-range value
//!   is refused, never replaced by the default. A float beyond 2^53 may not
//!   be the integer that was written, so such values (a large seed) must be
//!   written as integers.
//! - A float key accepts any number finite as an `f32`. One too large for an
//!   `f32` (`1e39`) is refused with "expects a finite number", as a live
//!   control write is.
//! - An absent key, or `null`, reads as absent (the default). A value of
//!   another JSON type (text, a boolean, an object) is refused.
//!
//! A factory declares each key once as a [`ConfigKey`] constant, reads it
//! through that constant and lists it in
//! [`ModuleFactory::config_keys`](crate::ModuleFactory::config_keys), so the
//! declaration and the read cannot diverge. Array elements and nested
//! values are read with [`whole_number`], [`whole_number_in`] and
//! [`finite_f32`], and refused through [`ConfigReader::refuse`].

use serde_json::Value;
use std::fmt;

#[cfg(test)]
pub(crate) mod tests;

/// The largest whole number every float spelling holds exactly.
const EXACT_FLOAT_LIMIT: f64 = 9_007_199_254_740_992.0; // 2^53

/// What a declared config key holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigKind {
    /// A whole number from `min` to `max`, inclusive.
    Integer { min: i128, max: i128 },
    /// A number that is finite as an `f32`.
    Float,
}

impl ConfigKind {
    /// Whether `a` and `b` read as one value of this kind, however each is
    /// spelled: `440` and `440.0` are one float, `72` and `72.0` one
    /// integer. An integer compares exactly, never through a float, and a
    /// float beyond 2^53 never equals an integer (the reader refuses it). A
    /// float compares as the `f32` the reader makes, bit for bit. A value
    /// the reader refuses is never the same as another.
    pub fn same_value(self, a: &Value, b: &Value) -> bool {
        match self {
            Self::Integer { min, max } => {
                let read = |value| exact_whole(value).filter(|whole| (min..=max).contains(whole));
                matches!((read(a), read(b)), (Some(a), Some(b)) if a == b)
            }
            Self::Float => matches!(
                (finite_f32(a), finite_f32(b)),
                (Ok(a), Ok(b)) if a.to_bits() == b.to_bits()
            ),
        }
    }
}

/// A numeric key a module's config may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigKey {
    /// The key, as the config's JSON object names it.
    pub key: &'static str,
    /// What the key holds.
    pub kind: ConfigKind,
}

impl ConfigKey {
    /// An integer key over the whole range of `T`.
    pub const fn int<T: ConfigInt>(key: &'static str) -> Self {
        Self::integer(key, T::MIN, T::MAX)
    }

    /// An integer key from `min` to `max`, for a module that already
    /// enforces a narrower range than its type's.
    pub const fn integer(key: &'static str, min: i128, max: i128) -> Self {
        Self {
            key,
            kind: ConfigKind::Integer { min, max },
        }
    }

    /// A seed: any `u64`. Seeds beyond 2^53 must be written as integers.
    pub const fn seed(key: &'static str) -> Self {
        Self::int::<u64>(key)
    }

    /// A float key: any number that is finite as an `f32`.
    pub const fn float(key: &'static str) -> Self {
        Self {
            key,
            kind: ConfigKind::Float,
        }
    }
}

/// An integer type a config key can be read as.
pub trait ConfigInt: Copy {
    /// The type's smallest value.
    const MIN: i128;
    /// The type's largest value.
    const MAX: i128;
    /// Converts a value already checked to lie in `MIN..=MAX`.
    fn from_checked(value: i128) -> Self;
}

macro_rules! config_int {
    ($($int:ty),*) => {$(
        impl ConfigInt for $int {
            const MIN: i128 = <$int>::MIN as i128;
            const MAX: i128 = <$int>::MAX as i128;
            fn from_checked(value: i128) -> Self {
                value as $int
            }
        }
    )*};
}

config_int!(u8, u16, u32, u64, usize, i8, i16, i32, i64);

/// Why a JSON value is not the number a key expects. Displays as
/// `expects {expected}, got {got}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberRefusal {
    /// What the key expects, e.g. `a finite number`.
    pub expected: String,
    /// The value as written (cut short when long).
    pub got: String,
}

impl NumberRefusal {
    fn new(expected: impl Into<String>, value: &Value) -> Self {
        let got = written(value);
        let expected = expected.into();
        Self { expected, got }
    }
}

impl fmt::Display for NumberRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "expects {}, got {}", self.expected, self.got)
    }
}

/// A config value a module type refused. Displays as
/// `{module_type} config '{key}' expects …, got …`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    /// The module type whose config it is.
    pub module_type: String,
    /// The key, or the path to an element (`levels[2]`).
    pub key: String,
    /// Why the value was refused.
    pub refusal: NumberRefusal,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} config '{}' {}",
            self.module_type, self.key, self.refusal
        )
    }
}

impl std::error::Error for ConfigError {}

/// Reads typed values from one module's config.
///
/// ```rust,ignore
/// const ROOT_NOTE: ConfigKey = ConfigKey::int::<u8>("root_note");
/// let reader = ConfigReader::new("melody", config);
/// let root_note = reader.int::<u8>(&ROOT_NOTE)?.unwrap_or(60);
/// ```
#[derive(Clone, Copy)]
pub struct ConfigReader<'a> {
    module_type: &'a str,
    config: &'a Value,
}

impl<'a> ConfigReader<'a> {
    /// A reader over `config`, naming `module_type` in its refusals.
    pub fn new(module_type: &'a str, config: &'a Value) -> Self {
        Self {
            module_type,
            config,
        }
    }

    /// The value at `key`, or `None` when it is absent or `null`.
    pub fn get(&self, key: &str) -> Option<&'a Value> {
        self.config.get(key).filter(|value| !value.is_null())
    }

    /// Reads integer key `key` as a `T`, or `None` when it is absent. The
    /// range is the key's declared range within `T`'s.
    pub fn int<T: ConfigInt>(&self, key: &ConfigKey) -> Result<Option<T>, ConfigError> {
        let (min, max) = match key.kind {
            ConfigKind::Integer { min, max } => (min, max),
            ConfigKind::Float => {
                debug_assert!(false, "'{}' is declared a float key", key.key);
                (T::MIN, T::MAX)
            }
        };
        self.get(key.key)
            .map(|value| whole_number_in(value, min, max).map_err(|r| self.refuse(key.key, r)))
            .transpose()
    }

    /// Reads float key `key`, or `None` when it is absent.
    pub fn float(&self, key: &ConfigKey) -> Result<Option<f32>, ConfigError> {
        debug_assert!(
            key.kind == ConfigKind::Float,
            "'{}' is declared an integer key",
            key.key
        );
        self.get(key.key)
            .map(|value| finite_f32(value).map_err(|r| self.refuse(key.key, r)))
            .transpose()
    }

    /// The refusal of the value at `key`, which may be a path to an element
    /// read with the value-level helpers (`levels[2]`, `zones[0].root`).
    pub fn refuse(&self, key: &str, refusal: NumberRefusal) -> ConfigError {
        ConfigError {
            module_type: self.module_type.to_string(),
            key: key.to_string(),
            refusal,
        }
    }
}

/// Reads `value` as a whole number over the whole range of `T`.
pub fn whole_number<T: ConfigInt>(value: &Value) -> Result<T, NumberRefusal> {
    whole_number_in(value, T::MIN, T::MAX)
}

/// Reads `value` as a whole number from `min` to `max` (within `T`'s
/// range): a JSON integer, or a float that is exactly whole, in range and
/// no larger than 2^53.
pub fn whole_number_in<T: ConfigInt>(
    value: &Value,
    min: i128,
    max: i128,
) -> Result<T, NumberRefusal> {
    let (min, max) = (min.max(T::MIN), max.min(T::MAX));
    let expected = format!("a whole number from {min} to {max}");
    let whole = match value {
        // serde_json holds only finite floats.
        Value::Number(number) if number.is_f64() => {
            let float = number.as_f64().unwrap_or(f64::NAN);
            let in_range = float >= min as f64 && float <= max as f64;
            if float.fract() == 0.0 && in_range && float.abs() > EXACT_FLOAT_LIMIT {
                let expected = format!("{expected}, written as an integer beyond 2^53");
                return Err(NumberRefusal::new(expected, value));
            }
            (float.fract() == 0.0 && in_range).then_some(float as i128)
        }
        Value::Number(number) => number
            .as_i64()
            .map(i128::from)
            .or(number.as_u64().map(i128::from)),
        _ => None,
    };
    match whole {
        Some(whole) if (min..=max).contains(&whole) => Ok(T::from_checked(whole)),
        _ => Err(NumberRefusal::new(expected, value)),
    }
}

/// `value` as the whole number an integer key reads it as, before its
/// range: a JSON integer, or a float that is exactly whole and no larger
/// than 2^53.
fn exact_whole(value: &Value) -> Option<i128> {
    let Value::Number(number) = value else {
        return None;
    };
    if number.is_f64() {
        let float = number.as_f64()?;
        return (float.fract() == 0.0 && float.abs() <= EXACT_FLOAT_LIMIT).then_some(float as i128);
    }
    number
        .as_i64()
        .map(i128::from)
        .or(number.as_u64().map(i128::from))
}

/// Reads `value` as a number that is finite as an `f32`; one too large for
/// an `f32` would become an infinity.
pub fn finite_f32(value: &Value) -> Result<f32, NumberRefusal> {
    match value.as_f64().map(|number| number as f32) {
        Some(narrowed) if narrowed.is_finite() => Ok(narrowed),
        _ => Err(NumberRefusal::new("a finite number", value)),
    }
}

/// `value` as its JSON, with exponents as people write them (`1e39`, not
/// `1e+39`), cut to about 40 bytes.
fn written(value: &Value) -> String {
    const LIMIT: usize = 40;
    let mut text = match value {
        Value::Number(number) => number.to_string().replace("e+", "e"),
        _ => value.to_string(),
    };
    if text.len() > LIMIT {
        let mut end = LIMIT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}
