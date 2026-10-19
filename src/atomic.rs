use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct AtomicF32 {
    bits: Arc<AtomicU32>,
}

impl AtomicF32 {
    pub(crate) fn new(value: f32) -> Self {
        Self {
            bits: Arc::new(AtomicU32::new(value.to_bits())),
        }
    }

    #[inline]
    pub(crate) fn load(&self) -> f32 {
        f32::from_bits(self.bits.load(Ordering::Relaxed))
    }

    #[inline]
    pub(crate) fn store(&self, value: f32) {
        self.bits.store(value.to_bits(), Ordering::Relaxed);
    }
}
