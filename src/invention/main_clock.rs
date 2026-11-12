//! The piece's main clock: which of its clocks leads where a surface shows
//! one, such as the status summary.
//!
//! Presentation only, like the layout: the field names a clock module's
//! id, is saved with the piece and builds nothing, so changing it rebuilds
//! nothing. A piece need not name one; then its first clock leads. An older
//! core does not know the field and drops it.

use super::Invention;

/// The module type a main clock must have.
const CLOCK_TYPE: &str = "clock";

impl Invention {
    /// The ids of the piece's clocks in the order a summary lists them: the
    /// main clock first when the piece names one, then the others in the
    /// order the piece lists them. A daemon takes as many from the front as
    /// it reports.
    pub fn summary_clocks(&self) -> Vec<&str> {
        let main = self.main_clock.as_deref();
        let clocks = self
            .modules
            .iter()
            .filter(|spec| spec.module_type == CLOCK_TYPE)
            .map(|spec| spec.id.as_str());
        let leader = clocks.clone().find(|id| Some(*id) == main);
        leader
            .into_iter()
            .chain(clocks.filter(|id| Some(*id) != leader))
            .collect()
    }

    /// Drops a main clock naming no clock of the piece, returning the
    /// warning that says so; the piece loads all the same.
    pub(crate) fn settle_main_clock(&mut self) -> Option<String> {
        let id = self.main_clock.as_deref()?;
        let module = self.modules.iter().find(|spec| spec.id == id);
        let why = match module {
            Some(spec) if spec.module_type == CLOCK_TYPE => return None,
            Some(spec) => format!("is a {}, not a clock", spec.module_type),
            None => "is not a module of the invention".to_string(),
        };
        let warning = format!("main_clock '{id}' {why}; it was dropped");
        self.main_clock = None;
        Some(warning)
    }
}

#[cfg(test)]
mod tests {
    use crate::Invention;

    fn piece(main_clock: Option<&str>) -> Invention {
        let mut invention: Invention = serde_json::from_value(serde_json::json!({
            "modules": [
                { "id": "a", "type": "clock" },
                { "id": "osc", "type": "oscillator" },
                { "id": "b", "type": "clock" }
            ],
            "connections": []
        }))
        .unwrap();
        invention.main_clock = main_clock.map(str::to_string);
        invention
    }

    #[test]
    fn the_main_clock_leads_then_the_rest_in_order() {
        assert_eq!(piece(None).summary_clocks(), ["a", "b"]);
        assert_eq!(piece(Some("b")).summary_clocks(), ["b", "a"]);
        assert_eq!(piece(Some("osc")).summary_clocks(), ["a", "b"]);
    }

    #[test]
    fn a_main_clock_naming_no_clock_is_dropped_with_a_warning() {
        let mut fine = piece(Some("b"));
        assert_eq!(fine.settle_main_clock(), None);
        assert_eq!(fine.main_clock.as_deref(), Some("b"));
        for id in ["osc", "ghost"] {
            let mut bad = piece(Some(id));
            let warning = bad.settle_main_clock().unwrap();
            assert!(warning.contains(&format!("'{id}'")), "{warning}");
            assert_eq!(bad.main_clock, None);
        }
    }

    #[test]
    fn removing_or_retyping_the_main_clock_forgets_it() {
        use crate::invention::authored_document::{remove_module, upsert_module};
        let config = serde_json::json!({});
        let mut retyped = piece(Some("b"));
        upsert_module(&mut retyped, "b", "clock", &config);
        assert_eq!(retyped.main_clock.as_deref(), Some("b"), "still a clock");
        upsert_module(&mut retyped, "b", "oscillator", &config);
        assert_eq!(retyped.main_clock, None);

        let mut removed = piece(Some("b"));
        remove_module(&mut removed, "a");
        assert_eq!(removed.main_clock.as_deref(), Some("b"));
        remove_module(&mut removed, "b");
        assert_eq!(removed.main_clock, None);
    }
}
