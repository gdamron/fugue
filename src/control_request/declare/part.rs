//! Controls that write one part of another: a cell sequencer's `cells.3`
//! is cell 3 of its `cells` bank. A part holds no value of its own. A write
//! to it is built, on a control thread, into a write of the whole it is
//! part of, from what the whole reads back now, and is made and recorded
//! as that write. So the module keeps the one payload it always kept, and
//! a document records the whole as a write of the whole would.
//!
//! A part is never listed: the whole lists the value, and the module's
//! description says how its parts are spelled.

use super::{ControlDecl, ControlTable};
use crate::ControlValue;

/// Builds a write of the whole from a write of one of its parts.
#[derive(Clone, Copy)]
pub(crate) struct WholePart {
    /// The control this is part of.
    pub(crate) whole: &'static str,
    /// The whole's new value from its value now, the part's index and
    /// the value written to the part, or why the part refuses it.
    pub(crate) assemble: fn(&ControlValue, u16, &ControlValue) -> Result<ControlValue, String>,
}

impl PartialEq for WholePart {
    fn eq(&self, other: &Self) -> bool {
        self.whole == other.whole && std::ptr::fn_addr_eq(self.assemble, other.assemble)
    }
}

impl std::fmt::Debug for WholePart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WholePart({})", self.whole)
    }
}

impl ControlDecl {
    /// Makes each index a part of control `whole`, written as `assemble`
    /// builds the whole (see [`WholePart`]).
    pub(crate) const fn part_of(
        mut self,
        whole: &'static str,
        assemble: fn(&ControlValue, u16, &ControlValue) -> Result<ControlValue, String>,
    ) -> Self {
        self.part = Some(WholePart { whole, assemble });
        self
    }
}

impl ControlTable {
    /// The write of the whole that a write of `value` to part `key` is,
    /// built from `current`, the whole's value now: `None` when `key` names
    /// no part. Control thread.
    pub(crate) fn whole_write(
        &self,
        key: &str,
        value: &ControlValue,
        current: impl FnOnce(&str) -> Option<ControlValue>,
    ) -> Option<Result<(String, ControlValue), String>> {
        let index = self.resolve(key)?;
        let (decl, position) = self.decl(index)?;
        let part = decl.part?;
        let Some(now) = current(part.whole) else {
            return Some(Err(format!("'{}' holds no value yet", part.whole)));
        };
        let built = (part.assemble)(&now, position, value);
        Some(built.map(|whole| (part.whole.to_string(), whole)))
    }
}

#[cfg(test)]
mod tests {
    use super::super::DeclKind;
    use super::*;
    use crate::control_request::RtValue;

    /// Appends the part's index and value to the whole's text.
    fn append(
        whole: &ControlValue,
        position: u16,
        part: &ControlValue,
    ) -> Result<ControlValue, String> {
        let part = part.as_string()?;
        if part.is_empty() {
            return Err("an empty part".into());
        }
        Ok(ControlValue::String(format!(
            "{}+{position}:{part}",
            whole.as_string()?
        )))
    }

    fn table() -> ControlTable {
        ControlTable::built(vec![
            ControlDecl::new("text", DeclKind::Payload, RtValue::Bool(false), "Whole"),
            ControlDecl::new("text", DeclKind::Payload, RtValue::Bool(false), "Part")
                .indexed(4)
                .part_of("text", append),
        ])
        .unwrap()
    }

    #[test]
    fn a_part_write_is_a_write_of_the_whole_built_from_its_value_now() {
        let table = table();
        let now = |key: &str| (key == "text").then(|| ControlValue::String("ab".into()));
        let value = ControlValue::String("c".into());
        let write = table.whole_write("text.2", &value, now).unwrap();
        assert_eq!(
            write,
            Ok(("text".into(), ControlValue::String("ab+2:c".into())))
        );
    }

    #[test]
    fn a_part_refuses_what_its_whole_cannot_take_and_needs_a_whole_to_write() {
        let table = table();
        let empty = ControlValue::String(String::new());
        let now = |_: &str| Some(ControlValue::String("ab".into()));
        assert_eq!(
            table.whole_write("text.0", &empty, now),
            Some(Err("an empty part".into()))
        );
        let none = table.whole_write("text.0", &ControlValue::String("c".into()), |_| None);
        assert!(none.unwrap().unwrap_err().contains("holds no value"));
    }

    #[test]
    fn a_whole_and_other_keys_are_no_parts_and_parts_are_never_listed() {
        let table = table();
        let value = ControlValue::String("c".into());
        assert!(table.whole_write("text", &value, |_| None).is_none());
        assert!(table.whole_write("text.4", &value, |_| None).is_none());
        let listed: Vec<String> = table
            .metas(|_| None)
            .into_iter()
            .map(|meta| meta.key)
            .collect();
        assert_eq!(listed, ["text"]);
    }
}
