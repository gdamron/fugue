//! Flush-to-zero for the audio thread.
//!
//! A decaying filter, reverb or envelope fed silence never reaches 0.0 in
//! `f32`: it settles on subnormal numbers, which some CPUs process many
//! times slower than normal ones, so a quiet tail can spike the audio
//! thread's load until it drops out. With flush-to-zero set, the FPU writes
//! a subnormal result as zero, and with denormals-are-zero it reads a
//! subnormal input as zero too.
//!
//! `SignalGraph::process_block` runs inside a [`FlushToZeroScope`], so every
//! block rendered, live or offline, has the mode set: aarch64's FPCR FZ bit
//! (which covers inputs and results alike), and x86_64's MXCSR FTZ and DAZ
//! bits. The scope restores the thread's previous mode when it ends, so an
//! offline render leaves its caller's thread as it found it. Other targets
//! compile it as a no-op.
//!
//! Only subnormal values change: every normal result is bit-identical.

use std::marker::PhantomData;

#[cfg(target_arch = "aarch64")]
mod arch {
    /// The FPCR word.
    pub(super) type Word = u64;
    /// FPCR.FZ: subnormal inputs and results are flushed to zero, for scalar
    /// and SIMD arithmetic alike.
    pub(super) const FLUSH: Word = 1 << 24;

    #[inline]
    pub(super) fn read() -> Word {
        let word: Word;
        // SAFETY: reading FPCR has no side effects.
        unsafe {
            core::arch::asm!("mrs {}, fpcr", out(reg) word, options(nostack, preserves_flags))
        };
        word
    }

    #[inline]
    pub(super) fn write(word: Word) {
        // SAFETY: FPCR only controls floating-point rounding and exception
        // modes for this thread; `word` is a value read from it with mode
        // bits changed.
        unsafe {
            core::arch::asm!("msr fpcr, {}", in(reg) word, options(nostack, preserves_flags))
        };
    }
}

#[cfg(target_arch = "x86_64")]
mod arch {
    /// The MXCSR word.
    pub(super) type Word = u32;
    /// MXCSR.FTZ (subnormal results become zero) and MXCSR.DAZ (subnormal
    /// inputs read as zero). Every x86_64 processor supports both.
    pub(super) const FLUSH: Word = (1 << 15) | (1 << 6);

    #[inline]
    pub(super) fn read() -> Word {
        let mut word: Word = 0;
        // SAFETY: stmxcsr stores the 32-bit MXCSR into `word`.
        unsafe {
            core::arch::asm!("stmxcsr [{}]", in(reg) &mut word, options(nostack, preserves_flags))
        };
        word
    }

    #[inline]
    pub(super) fn write(word: Word) {
        // SAFETY: ldmxcsr loads MXCSR from `word`, a value read from it with
        // the FTZ/DAZ bits changed, so no reserved bit is set.
        unsafe {
            core::arch::asm!("ldmxcsr [{}]", in(reg) &word, options(nostack, readonly, preserves_flags))
        };
    }
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
mod arch {
    pub(super) type Word = u8;
    /// No mode to set: [`super::FlushToZeroScope`] does nothing here.
    pub(super) const FLUSH: Word = 0;

    #[inline]
    pub(super) fn read() -> Word {
        0
    }

    #[inline]
    pub(super) fn write(_word: Word) {}
}

/// Whether flush-to-zero (and denormals-are-zero, where the platform has it)
/// is set on the current thread. It is while a block renders, live or
/// offline, so a module's `process` sees `true` on aarch64 and x86_64.
/// Always `false` on other targets.
#[inline]
pub fn flush_to_zero_enabled() -> bool {
    arch::FLUSH != 0 && arch::read() & arch::FLUSH == arch::FLUSH
}

/// Sets flush-to-zero on the current thread until dropped, then restores the
/// mode it found. Writes the register only when the mode changes, so nested
/// scopes cost one read each. Never allocates.
#[must_use = "the mode is restored when this value is dropped"]
pub(crate) struct FlushToZeroScope {
    /// The mode to restore, or `None` when it was already set.
    previous: Option<arch::Word>,
    /// Tied to the thread whose mode it set.
    _not_send: PhantomData<*const ()>,
}

impl FlushToZeroScope {
    #[inline]
    pub(crate) fn enter() -> Self {
        let word = arch::read();
        let previous = (word & arch::FLUSH != arch::FLUSH).then(|| {
            arch::write(word | arch::FLUSH);
            word
        });
        Self {
            previous,
            _not_send: PhantomData,
        }
    }
}

impl Drop for FlushToZeroScope {
    #[inline]
    fn drop(&mut self) {
        if let Some(word) = self.previous {
            arch::write(word);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hint::black_box;

    const SUPPORTED: bool = cfg!(any(target_arch = "aarch64", target_arch = "x86_64"));

    /// A one-pole decay fed silence, as a filter or reverb tail is: from
    /// full scale, 140 halvings pass well below the smallest normal
    /// `f32`.
    fn decay_tail() -> f32 {
        let mut state = black_box(1.0f32);
        let coefficient = black_box(0.5f32);
        for _ in 0..140 {
            state = black_box(state * coefficient + black_box(0.0));
        }
        state
    }

    #[test]
    fn a_decaying_path_reaches_zero_with_the_mode_set() {
        let without = decay_tail();
        assert!(without.is_subnormal(), "{without:e}");
        let with = {
            let _scope = FlushToZeroScope::enter();
            decay_tail()
        };
        if SUPPORTED {
            assert_eq!(with.to_bits(), 0, "{with:e}");
        } else {
            assert_eq!(with.to_bits(), without.to_bits());
        }
    }

    #[test]
    fn a_subnormal_input_reads_as_zero_with_the_mode_set() {
        let subnormal = black_box(f32::MIN_POSITIVE / 4.0);
        assert!(subnormal.is_subnormal());
        let _scope = FlushToZeroScope::enter();
        let passed = black_box(subnormal) * black_box(1.0f32);
        if SUPPORTED {
            assert_eq!(passed, 0.0);
        }
    }

    #[test]
    fn the_scope_sets_the_mode_and_restores_the_one_it_found() {
        assert!(!flush_to_zero_enabled());
        {
            let _outer = FlushToZeroScope::enter();
            assert_eq!(flush_to_zero_enabled(), SUPPORTED);
            {
                let _inner = FlushToZeroScope::enter();
                assert_eq!(flush_to_zero_enabled(), SUPPORTED);
            }
            // The inner scope found it set, so it leaves it set.
            assert_eq!(flush_to_zero_enabled(), SUPPORTED);
        }
        assert!(!flush_to_zero_enabled());
        // Normal arithmetic is untouched.
        let product = black_box(0.3f32) * black_box(0.7f32);
        assert_eq!(product.to_bits(), (0.3f32 * 0.7f32).to_bits());
    }
}
