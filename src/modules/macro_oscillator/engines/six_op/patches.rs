//! The 32 patches `harmonics` chooses between, in order.

use super::patch::Patch;
use super::patches_held::{HELD, PLUCKS};
use super::patches_struck::{BELLS, KEYS};

/// How many patches there are.
pub(in crate::modules::macro_oscillator) const PATCH_COUNT: usize = 32;

/// Keys, bells and mallets, basses and plucks, then organs, pads, winds
/// and brass, eight of each.
pub(in crate::modules::macro_oscillator) static PATCHES: [Patch; PATCH_COUNT] = {
    let mut all = [KEYS[0]; PATCH_COUNT];
    let mut i = 0;
    while i < 8 {
        all[i] = KEYS[i];
        all[8 + i] = BELLS[i];
        all[16 + i] = PLUCKS[i];
        all[24 + i] = HELD[i];
        i += 1;
    }
    all
};

#[cfg(test)]
mod tests {
    use super::super::patch::problem;
    use super::*;

    #[test]
    fn every_patch_is_playable() {
        for patch in &PATCHES {
            assert_eq!(problem(patch), None);
        }
    }

    #[test]
    #[rustfmt::skip]
    fn the_names_are_unique_and_in_order() {
        let names: Vec<&str> = PATCHES.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            [
                "glass piano", "soft tine keys", "bright tine keys", "reed keys",
                "funky pluck keys", "quill keys", "toy piano", "music box",
                "tubular bell", "glockenspiel", "vibraphone", "marimba",
                "kalimba", "steel pan", "gong", "crystal bell",
                "solid bass", "slap bass", "rubber bass", "sub pluck",
                "harp", "plucked silk", "nylon guitar", "twang",
                "drawbar organ", "reed organ", "warm pad", "glass pad",
                "brass section", "flute", "reed lead", "metal drone",
            ]
        );
    }
}
