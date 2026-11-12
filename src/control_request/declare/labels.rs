//! A declared control's labels and preferred editor: presentation only, as
//! hints are (see [`crate::traits::labels`]). They never change what a
//! control accepts or how a write names it.

use std::collections::BTreeMap;

use super::{same_bytes, ControlDecl, DeclKind};
use crate::traits::labels::{listed_label, listed_labels};
use crate::ControlWidget;

impl ControlDecl {
    /// What a person reads for it, when tidying its key says it badly
    /// ("Pulse width" for `pw`).
    pub(crate) const fn label(mut self, label: &'static str) -> Self {
        self.label = Some(label);
        self
    }

    /// Labels of a choice's options, each paired with its option
    /// (`("lowpass", "Low-pass")`).
    pub(crate) const fn option_labels(
        mut self,
        labels: &'static [(&'static str, &'static str)],
    ) -> Self {
        self.option_labels = labels;
        self
    }

    /// The editor its module prefers for it.
    pub(crate) const fn widget(mut self, widget: ControlWidget) -> Self {
        self.widget = Some(widget);
        self
    }

    /// Its label as listed for `key`, index `position` of it: an indexed
    /// control's label names the index, as its key does. `None` when it
    /// says no more than the tidied key.
    pub(super) fn listed_label(&self, key: &str, position: u16) -> Option<String> {
        let label = self.label?;
        if self.indexed {
            return listed_label(key, Some(&format!("{label} {position}")));
        }
        listed_label(key, Some(label))
    }

    /// Its option labels that say more than the tidied option.
    pub(super) fn listed_option_labels(&self) -> BTreeMap<String, String> {
        listed_labels(self.option_labels)
    }
}

/// Why `decl`'s labels or editor cannot stand: a blank label, option
/// labels on a control that is not a choice or for an option it does not
/// offer, or an editor its kind cannot use (a knob, slider or stepper
/// shows a number, a segmented control or menu a choice).
pub(super) const fn invalid(decl: &ControlDecl) -> Option<&'static str> {
    if let Some(label) = decl.label {
        if blank(label) {
            return Some("a label must not be blank");
        }
    }
    // A control its module's own surface reads lists as free text.
    if decl.reader.is_some() && (!decl.option_labels.is_empty() || decl.widget.is_some()) {
        return Some("a control read as text has no option labels or editor");
    }
    let options: &[&str] = match decl.kind {
        DeclKind::Choice(options) => options,
        _ if !decl.option_labels.is_empty() => {
            return Some("only a choice control labels its options")
        }
        _ => &[],
    };
    let mut i = 0;
    while i < decl.option_labels.len() {
        let (option, label) = decl.option_labels[i];
        if blank(label) {
            return Some("a label must not be blank");
        }
        let mut offered = false;
        let mut j = 0;
        while j < options.len() {
            offered = offered || same_bytes(options[j].as_bytes(), option.as_bytes());
            j += 1;
        }
        if !offered {
            return Some("an option label must name an option the choice offers");
        }
        i += 1;
    }
    let number = matches!(
        decl.kind,
        DeclKind::Number { .. } | DeclKind::Integer { .. }
    );
    let choice = matches!(decl.kind, DeclKind::Choice(_));
    match decl.widget {
        Some(ControlWidget::Knob | ControlWidget::Slider | ControlWidget::Stepper) if !number => {
            Some("a knob, slider or stepper shows a number")
        }
        Some(ControlWidget::Segmented | ControlWidget::Menu) if !choice => {
            Some("a segmented control or menu shows a choice")
        }
        _ => None,
    }
}

/// Whether `label` is empty or only ASCII whitespace.
const fn blank(label: &str) -> bool {
    let bytes = label.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_whitespace() {
            return false;
        }
        i += 1;
    }
    true
}

#[cfg(test)]
mod tests;
