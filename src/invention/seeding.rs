//! An invention-wide seed for the random sources a document leaves unseeded.
//!
//! A melody, chance or noise module with no seed draws one from entropy as
//! it is built, so the piece plays differently every time: that is what
//! the examples intend. A render that must repeat (a test, a before/after
//! comparison) builds under an invention seed instead
//! ([`InventionBuilder::with_seed`](super::InventionBuilder::with_seed),
//! [`RenderEngine::set_seed`](super::RenderEngine::set_seed)): each module
//! that would have drawn from entropy draws from a seed derived from the
//! invention seed and the module's id instead, and the document is left as
//! written. Modules with a seed of their own keep it.
//!
//! The derived seed depends only on the invention seed and the module's id
//! (within a development, the instance's id and then the inner module's),
//! so adding or removing an unrelated module leaves every other module's
//! stream as it was. Builds run on control threads; the seed in force is
//! held per thread for the length of one build.

use std::cell::RefCell;

use rand::rngs::StdRng;
use rand::RngExt;

/// The seed a build is under and the ids of the modules it is inside.
struct Scope {
    seed: u64,
    path: Vec<String>,
}

thread_local! {
    static SCOPE: RefCell<Option<Scope>> = const { RefCell::new(None) };
}

/// Runs `build` with `seed` as the invention seed; `None` leaves the seed
/// in force (none, or that of the build this one is nested in).
pub(crate) fn seeded<R>(seed: Option<u64>, build: impl FnOnce() -> R) -> R {
    let Some(seed) = seed else {
        return build();
    };
    let previous = SCOPE.with(|scope| {
        scope.replace(Some(Scope {
            seed,
            path: Vec::new(),
        }))
    });
    let _restore = Restore(Some(previous));
    build()
}

/// Runs `build` for the module `id`: a random source built inside it draws
/// from the seed derived for it. Without an invention seed, nothing changes.
pub(crate) fn within_module<R>(id: &str, build: impl FnOnce() -> R) -> R {
    let entered = SCOPE.with(|scope| match scope.borrow_mut().as_mut() {
        Some(scope) => {
            scope.path.push(id.to_string());
            true
        }
        None => false,
    });
    let _leave = Leave(entered);
    build()
}

/// The seed for a random source that has none of its own: derived from the
/// invention seed and the module being built, or drawn from entropy when the
/// build is under no invention seed.
pub(crate) fn entropy() -> u64 {
    let derived = SCOPE.with(|scope| {
        let scope = scope.borrow();
        let scope = scope.as_ref()?;
        Some(derive(scope.seed, &scope.path))
    });
    derived.unwrap_or_else(|| {
        let mut entropy: StdRng = rand::make_rng();
        entropy.random()
    })
}

/// The seed derived for the module at `path` (outermost id first) under
/// the invention seed `seed`: FNV-1a over the seed and each id, finished
/// with SplitMix64's mixer so nearby inputs land far apart. Stable across
/// platforms; streams are stable within a build (see [`crate::dsp::Rng`]).
pub(crate) fn derive<S: AsRef<str>>(seed: u64, path: &[S]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    let mut eat = |byte: u8| hash = (hash ^ u64::from(byte)).wrapping_mul(PRIME);
    seed.to_le_bytes().into_iter().for_each(&mut eat);
    for id in path {
        // A separator no id holds, so ["ab", "c"] and ["a", "bc"] differ.
        eat(0);
        id.as_ref().bytes().for_each(&mut eat);
    }
    let mut z = hash.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Puts the scope back as it was when a seeded build ends, even by panic.
struct Restore(Option<Option<Scope>>);

impl Drop for Restore {
    fn drop(&mut self) {
        if let Some(previous) = self.0.take() {
            SCOPE.with(|scope| *scope.borrow_mut() = previous);
        }
    }
}

/// Leaves a module's scope when its build ends, even by panic.
struct Leave(bool);

impl Drop for Leave {
    fn drop(&mut self) {
        if self.0 {
            SCOPE.with(|scope| {
                if let Some(scope) = scope.borrow_mut().as_mut() {
                    scope.path.pop();
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_and_an_id_fix_the_derived_seed() {
        let first = seeded(Some(7), || within_module("mel", entropy));
        let again = seeded(Some(7), || within_module("mel", entropy));
        assert_eq!(first, again);
        assert_eq!(first, derive(7, &["mel"]));
    }

    #[test]
    fn other_seeds_and_ids_derive_other_seeds() {
        let base = derive(7, &["mel"]);
        assert_ne!(base, derive(8, &["mel"]));
        assert_ne!(base, derive(7, &["mel2"]));
        assert_ne!(derive(7, &["ab", "c"]), derive(7, &["a", "bc"]));
        // A module inside a development is told apart by its instance.
        assert_ne!(
            derive(7, &["voice_1", "mel"]),
            derive(7, &["voice_2", "mel"])
        );
    }

    #[test]
    fn without_a_seed_sources_draw_from_entropy() {
        let first = within_module("mel", entropy);
        let second = within_module("mel", entropy);
        assert_ne!(first, second);
        // A seeded build leaves no seed behind.
        seeded(Some(1), || {});
        assert_ne!(within_module("mel", entropy), derive(1, &["mel"]));
    }

    #[test]
    fn a_nested_build_keeps_the_seed_in_force() {
        let nested = seeded(Some(3), || {
            within_module("voice", || seeded(None, || within_module("mel", entropy)))
        });
        assert_eq!(nested, derive(3, &["voice", "mel"]));
    }
}
