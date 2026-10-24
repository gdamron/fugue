//! Indices past a fixed count: an indexed control listed below a read-only
//! count (a mixer's `level.N` below its `channel_count`) has no index at or
//! past it. Nothing can write such a count, so those indices could never be
//! listed or heard: they are refused as unknown, saying why, wherever a
//! control thread resolves a key. Below a count a client writes (a melody's
//! `degree_count`), every index still exists, listed or not.

use super::{ControlIndex, ControlTable, RtValue, Writer};

impl ControlTable {
    /// The control `key` names, unless it does not exist: unknown, or an
    /// index at or past the read-only count it is listed below, as
    /// `current` reads that count. The refusal says which.
    pub(crate) fn resolve_present(
        &self,
        key: &str,
        current: impl Fn(ControlIndex) -> Option<RtValue>,
    ) -> Result<ControlIndex, String> {
        let index = self
            .resolve(key)
            .ok_or_else(|| format!("Unknown control: {key}"))?;
        match self.past_fixed_count(index, current) {
            Some(why) => Err(why),
            None => Ok(index),
        }
    }

    /// Why `index` does not exist, when it is at or past the read-only
    /// count its control is listed below; `None` otherwise, and when the
    /// count cannot be read.
    fn past_fixed_count(
        &self,
        index: ControlIndex,
        current: impl Fn(ControlIndex) -> Option<RtValue>,
    ) -> Option<String> {
        let (decl, position) = self.decl(index)?;
        let count_key = decl.listed_below?;
        let count_index = self.resolve(count_key)?;
        let (count_decl, _) = self.decl(count_index)?;
        if count_decl.writer != Writer::Telemetry {
            return None;
        }
        let count = self.value(count_index, current(count_index)?)?;
        let count = count.as_number().ok()?;
        if f32::from(position) < count {
            return None;
        }
        let key = &decl.key;
        let has = match count as u32 {
            0 => format!("no '{key}' exists"),
            1 => format!("only '{key}.0' exists"),
            count => format!("only '{key}.0' to '{key}.{}' exist", count - 1),
        };
        Some(format!(
            "Unknown control: {key}.{position}: {count_key} is {count}, so {has}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{ControlDecl, DeclKind};
    use super::*;

    /// Slots below a fixed `size`, and positions below a `count` clients
    /// write.
    fn table() -> ControlTable {
        let number = DeclKind::Number { min: 0.0, max: 8.0 };
        ControlTable::built(vec![
            ControlDecl::new("size", number, RtValue::F32(2.0), "Size").telemetry(),
            ControlDecl::new("count", number, RtValue::F32(2.0), "Count"),
            ControlDecl::new("slot", number, RtValue::F32(0.0), "Slot")
                .indexed(4)
                .listed_below("size"),
            ControlDecl::new("pos", number, RtValue::F32(0.0), "Position")
                .indexed(4)
                .listed_below("count"),
        ])
        .unwrap()
    }

    fn holding(size: f32) -> impl Fn(ControlIndex) -> Option<RtValue> {
        move |index| {
            Some(RtValue::F32(if index == ControlIndex(0) {
                size
            } else {
                2.0
            }))
        }
    }

    #[test]
    fn an_index_past_a_read_only_count_does_not_exist_and_says_why() {
        let table = table();
        assert_eq!(
            table.resolve_present("slot.1", holding(2.0)),
            Ok(ControlIndex(3))
        );
        assert_eq!(
            table.resolve_present("slot.2", holding(2.0)),
            Err("Unknown control: slot.2: size is 2, so only 'slot.0' to 'slot.1' exist".into())
        );
        assert_eq!(
            table.resolve_present("slot.1", holding(1.0)),
            Err("Unknown control: slot.1: size is 1, so only 'slot.0' exists".into())
        );
        assert!(table.resolve_present("slot.3", holding(4.0)).is_ok());
        assert_eq!(
            table.resolve_present("slot.4", holding(4.0)),
            Err("Unknown control: slot.4".into()),
            "past the table, as ever"
        );
    }

    #[test]
    fn an_index_past_a_count_clients_write_still_exists() {
        let table = table();
        assert!(table.resolve_present("pos.3", holding(2.0)).is_ok());
        assert!(table.resolve_present("size", holding(2.0)).is_ok());
    }

    #[test]
    fn an_unreadable_count_bounds_nothing() {
        let table = table();
        assert!(table.resolve_present("slot.3", |_| None).is_ok());
    }
}
