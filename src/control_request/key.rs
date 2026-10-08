//! Compile-time-checked keys into a module's control table.

use std::marker::PhantomData;

use super::request::{ControlIndex, RtValue};

/// A Rust type carried by one [`RtValue`] variant. (Not named
/// `ControlValue`: the crate already exports that for the wire.)
pub(crate) trait RtScalar: Copy + Send + 'static {
    fn into_rt(self) -> RtValue;
    /// `None` when `value` holds another type.
    fn from_rt(value: RtValue) -> Option<Self>;
}

macro_rules! rt_scalar {
    ($($ty:ty => $variant:ident),*) => {$(
        impl RtScalar for $ty {
            fn into_rt(self) -> RtValue {
                RtValue::$variant(self)
            }
            fn from_rt(value: RtValue) -> Option<Self> {
                match value {
                    RtValue::$variant(value) => Some(value),
                    _ => None,
                }
            }
        }
    )*};
}

rt_scalar!(f32 => F32, u32 => U32, i32 => I32, bool => Bool);

/// The position of one `T`-typed control in a module's table of `N`
/// controls.
///
/// Construction checks `index < N`, so an out-of-range `const` key fails to
/// compile (static key validity, R5).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ControlKey<T, const N: usize> {
    index: ControlIndex,
    _type: PhantomData<fn() -> T>,
}

impl<T: RtScalar, const N: usize> ControlKey<T, N> {
    /// # Panics
    ///
    /// If `index >= N` or `N` exceeds a [`ControlIndex`]: a compile error
    /// when evaluated in a `const`.
    pub(crate) const fn new(index: usize) -> Self {
        assert!(N <= 1 << 16, "control table too large");
        assert!(index < N, "control index out of range");
        Self {
            index: ControlIndex(index as u16),
            _type: PhantomData,
        }
    }

    pub(crate) const fn index(self) -> ControlIndex {
        self.index
    }
}

/// `len` consecutive `T`-typed controls of a table of `N` from `start`, so a
/// module can name a table once:
/// `const DEGREES: ControlKeys<i32, CONTROLS> = ControlKeys::new(0, MAX_DEGREES);`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ControlKeys<T, const N: usize> {
    start: usize,
    len: usize,
    _type: PhantomData<fn() -> T>,
}

impl<T: RtScalar, const N: usize> ControlKeys<T, N> {
    /// # Panics
    ///
    /// If the range ends past `N` or `N` exceeds a [`ControlIndex`]: a
    /// compile error when evaluated in a `const`.
    pub(crate) const fn new(start: usize, len: usize) -> Self {
        assert!(N <= 1 << 16, "control table too large");
        assert!(
            start <= N && len <= N - start,
            "control range out of bounds"
        );
        Self {
            start,
            len,
            _type: PhantomData,
        }
    }

    pub(crate) const fn len(self) -> usize {
        self.len
    }

    /// The `i`th control of the range.
    ///
    /// # Panics
    ///
    /// If `i >= len`.
    pub(crate) const fn at(self, i: usize) -> ControlKey<T, N> {
        assert!(i < self.len, "control out of range");
        ControlKey::new(self.start + i)
    }
}
