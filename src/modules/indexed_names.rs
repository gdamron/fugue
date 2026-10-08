//! Names of an indexed port family (`audio.0`, `audio.1`, ...), generated
//! once instead of listed by hand.

use std::sync::OnceLock;

/// The first `count` names of the family `prefix.0` to `prefix.{max - 1}`.
///
/// A module's `inputs()` and `outputs()` borrow `&str`s that outlive any one
/// instance, so each family is built the first time it is asked for and kept
/// for the life of the process. There are at most a few hundred short names.
pub(crate) struct IndexedNames {
    prefix: &'static str,
    max: usize,
    names: OnceLock<Vec<&'static str>>,
}

impl IndexedNames {
    pub(crate) const fn new(prefix: &'static str, max: usize) -> Self {
        Self {
            prefix,
            max,
            names: OnceLock::new(),
        }
    }

    /// The first `count` names, clamped to the family's size.
    pub(crate) fn first(&'static self, count: usize) -> &'static [&'static str] {
        let names = self.names.get_or_init(|| {
            (0..self.max)
                .map(|index| &*Box::leak(format!("{}.{index}", self.prefix).into_boxed_str()))
                .collect()
        });
        &names[..count.min(self.max)]
    }

    /// The index in `prefix.N` when `port` names one of the first `count`.
    pub(crate) fn index_of(&'static self, port: &str, count: usize) -> Option<usize> {
        let index: usize = port
            .strip_prefix(self.prefix)?
            .strip_prefix('.')?
            .parse()
            .ok()?;
        // Only the canonical spelling counts: not `audio.01` or `audio.+1`.
        (self.first(count).get(index) == Some(&port)).then_some(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static FAMILY: IndexedNames = IndexedNames::new("audio", 4);

    #[test]
    fn names_count_from_zero() {
        assert_eq!(FAMILY.first(3), ["audio.0", "audio.1", "audio.2"]);
        assert_eq!(FAMILY.first(9).len(), 4);
        assert_eq!(FAMILY.index_of("audio.2", 3), Some(2));
    }

    #[test]
    fn only_the_canonical_spelling_within_count_matches() {
        assert_eq!(FAMILY.index_of("audio.3", 3), None);
        assert_eq!(FAMILY.index_of("audio.03", 4), None);
        assert_eq!(FAMILY.index_of("audio.1 ", 4), None);
        assert_eq!(FAMILY.index_of("audio1", 4), None);
    }
}
