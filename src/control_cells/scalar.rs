//! A single lock-free control value.

use std::marker::PhantomData;

use super::sync::{AtomicU32, Ordering};
use super::value::CellValue;

/// One control value that any thread can load or store without locking or
/// allocating.
///
/// Writer role: the owner declares each cell either a *parameter* (written
/// by control threads and schedulers, last writer wins) or *telemetry*
/// (written only by the module's own `process()`), never both (R1).
///
/// Ordering: stores are `Release` and loads `Acquire`, so a cell can publish
/// data written before it: a thread that loads a stored value also sees
/// everything the storing thread wrote before the store. This costs nothing
/// extra on x86 and little on ARM at control rates, and spares owners an
/// ordering argument of their own.
pub(crate) struct ScalarCell<T: CellValue> {
    bits: AtomicU32,
    _type: PhantomData<fn() -> T>,
}

impl<T: CellValue> ScalarCell<T> {
    pub(crate) fn new(value: T) -> Self {
        Self {
            bits: AtomicU32::new(value.to_bits()),
            _type: PhantomData,
        }
    }

    #[inline]
    pub(crate) fn load(&self) -> T {
        T::from_bits(self.bits.load(Ordering::Acquire))
    }

    #[inline]
    pub(crate) fn store(&self, value: T) {
        self.bits.store(value.to_bits(), Ordering::Release);
    }
}
