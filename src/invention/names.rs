//! What a piece calls its modules.
//!
//! A module's name ("Lead filter") is presentation, saved with the piece
//! like its [layout](super::layout): surfaces show it, and agents read it
//! beside the module's id, which stays what every command addresses. It
//! never changes the sound: no module is built from it, a reload whose only
//! difference is a name rebuilds nothing.
//!
//! An older core reads a document with names and plays it, ignoring the
//! field as it ignores any unknown one; saving there drops the names. That
//! costs nothing audible, so a name does not raise the document's
//! `minimum_core_version`.

use super::format::Invention;
use super::layout::{Layout, LayoutChange};

/// The longest module name, in UTF-8 bytes.
pub const MAX_MODULE_NAME_BYTES: usize = 256;

/// One change an edit batch makes to the names, in batch order: a module
/// named, or its name cleared (`None`, as `remove_module` leaves it).
pub(crate) type NameChange = (String, Option<String>);

/// Why `name` cannot name a module, if it cannot: it is blank, longer than
/// [`MAX_MODULE_NAME_BYTES`], or holds a control, separator or invisible
/// format character (a line break would break the one-line text an agent
/// reads, and a direction override would disguise one name as another).
pub(crate) fn name_refusal(name: &str) -> Option<String> {
    if name.trim().is_empty() {
        return Some("a module name must not be blank".to_string());
    }
    if name.len() > MAX_MODULE_NAME_BYTES {
        return Some(format!(
            "a module name must be at most {MAX_MODULE_NAME_BYTES} bytes"
        ));
    }
    if name.chars().any(breaks_the_line) {
        return Some("a module name must not hold control characters".to_string());
    }
    None
}

/// Whether `c` would break or disguise the one-line text a name is read in:
/// a control character, a line or paragraph separator, or a bidirectional
/// or zero-width format character.
fn breaks_the_line(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{2028}' | '\u{2029}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
        )
}

/// The presentation changes an edit batch or a reload makes: where modules
/// sit and what they are called. Made again on the retained document as it
/// stands when the change lands (see [`Invention::carry_presentation`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PresentationChanges {
    pub(crate) layout: Vec<LayoutChange>,
    pub(crate) names: Vec<NameChange>,
}

impl PresentationChanges {
    /// What turns `base` into `document`'s presentation: its layout's
    /// changes, and each of its modules whose name differs from what
    /// `base` calls the module with that id.
    pub(crate) fn between(base: Option<&Invention>, document: &Invention) -> Self {
        let empty = Layout::default();
        let base_layout = base.map_or(&empty, |base| &base.layout);
        let names = (document.modules.iter())
            .filter(|spec| base.and_then(|base| name_of(base, &spec.id)) != spec.name.as_deref())
            .map(|spec| (spec.id.clone(), spec.name.clone()))
            .collect();
        Self {
            layout: document.layout.changes_from(base_layout),
            names,
        }
    }
}

fn name_of<'a>(document: &'a Invention, id: &str) -> Option<&'a str> {
    (document.modules.iter())
        .find(|spec| spec.id == id)
        .and_then(|spec| spec.name.as_deref())
}

impl Invention {
    /// Takes the presentation `current`, the retained document as this
    /// document replaces it, has, with `changes` made on top: its layout
    /// (see [`Self::carry_layout`]) and, for each module this document
    /// has, the name `current` gives it. A batch's document was edited
    /// from the document as the batch read it; a module named since (while
    /// a timed batch waited, say) keeps its name.
    pub(crate) fn carry_presentation(
        &mut self,
        current: &Invention,
        changes: &PresentationChanges,
    ) {
        self.carry_layout(&current.layout, &changes.layout);
        for spec in &mut self.modules {
            spec.name = name_of(current, &spec.id).map(str::to_string);
        }
        for (id, name) in &changes.names {
            if let Some(spec) = self.modules.iter_mut().find(|spec| &spec.id == id) {
                spec.name = name.clone();
            }
        }
    }

    /// Drops what cannot be presented: layout entries that place no module
    /// (see [`Self::settle_layout`]), a main clock naming no clock of the
    /// piece (see [`Self::settle_main_clock`]) and names that cannot name one (see
    /// [`name_refusal`]). Returns a message for each, for the load to report
    /// as a warning; the piece loads all the same.
    pub(crate) fn settle_presentation(&mut self) -> Vec<String> {
        let mut dropped = self.settle_layout();
        dropped.extend(self.settle_main_clock());
        for spec in &mut self.modules {
            let Some(reason) = spec.name.as_deref().and_then(name_refusal) else {
                continue;
            };
            dropped.push(format!(
                "module '{}' has a name that cannot be used ({reason}); it was dropped",
                spec.id
            ));
            spec.name = None;
        }
        dropped
    }
}

#[cfg(test)]
#[path = "names_tests.rs"]
mod tests;
