//! Where an invention's modules sit on the canvas that shows the graph.
//!
//! A layout is presentation, saved with the piece so every surface (and
//! every agent) sees the same arrangement. It never changes the sound: no
//! module is built from it, a reload whose only difference is the layout
//! rebuilds nothing, and moving a module (`move_modules`) changes only the
//! document.
//!
//! An older core reads a document with a layout and plays it, ignoring the
//! field as it ignores any unknown one; saving there drops the arrangement.
//! That costs nothing audible, so a layout does not raise the document's
//! `minimum_core_version`: refusing to play a piece over where its boxes sit
//! would cost the composer more than the arrangement is worth.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::format::Invention;

/// The arrangement of an invention's own top-level modules.
///
/// In a document: `"layout": { "modules": { "osc": { "x": 120, "y": 40 } } }`.
/// A module with no entry has no saved position; the surface places it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct Layout {
    /// Each placed module's position, by module id. Ordered, so a saved
    /// document is deterministic.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub modules: BTreeMap<String, Position>,
}

impl Layout {
    /// Whether the layout places nothing; such a layout is not saved.
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
}

/// A point on the canvas, in abstract canvas units the surface scales.
/// Both coordinates are finite.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

impl Position {
    /// Whether both coordinates are finite numbers, as a document holds.
    pub fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// One change an edit batch makes to the layout, in batch order: a module
/// placed, or unplaced (`None`, as `move_modules` with `null` or
/// `remove_module` leaves it).
pub(crate) type LayoutChange = (String, Option<Position>);

impl Invention {
    /// Takes `current`, the layout the retained document has as this
    /// document replaces it, with `changes` made on top, keeping only the
    /// modules this document has. A batch's document was edited from the
    /// layout as the batch read it; a module moved since (while a timed
    /// batch waited, say) keeps where it was moved.
    pub(crate) fn carry_layout(&mut self, current: &Layout, changes: &[LayoutChange]) {
        let mut layout = current.clone();
        for (id, position) in changes {
            match position {
                Some(position) => layout.modules.insert(id.clone(), *position),
                None => layout.modules.remove(id),
            };
        }
        let modules = &self.modules;
        layout
            .modules
            .retain(|id, _| modules.iter().any(|spec| &spec.id == id));
        self.layout = layout;
    }

    /// Drops the layout entries that cannot place a module: one naming no
    /// module in the document, or one whose position is not finite. Returns
    /// a message for each, for the load to report as a warning; the piece
    /// loads all the same.
    ///
    /// JSON has no non-finite numbers, so a parsed document only meets the
    /// second case when built in code.
    pub(crate) fn settle_layout(&mut self) -> Vec<String> {
        if self.layout.is_empty() {
            return Vec::new();
        }
        let modules = &self.modules;
        let mut dropped = Vec::new();
        self.layout.modules.retain(|id, position| {
            if !modules.iter().any(|spec| &spec.id == id) {
                dropped.push(format!(
                    "layout places module '{id}', which the invention does not have; \
                     its position was dropped"
                ));
                return false;
            }
            if !position.is_finite() {
                dropped.push(format!(
                    "layout places module '{id}' at a position that is not finite; \
                     its position was dropped"
                ));
                return false;
            }
            true
        });
        dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLACED: &str = r#"{
        "modules": [{ "id": "osc", "type": "oscillator" }, { "id": "dac", "type": "dac" }],
        "connections": [],
        "layout": { "modules": { "osc": { "x": 120.5, "y": -40 }, "dac": { "x": 300, "y": 0 } } }
    }"#;

    #[test]
    fn a_document_without_a_layout_saves_without_one() {
        let json = r#"{"modules":[{"id":"osc","type":"oscillator"}],"connections":[]}"#;
        let document = Invention::from_json(json).unwrap();
        assert!(document.layout.is_empty());
        let saved = document.to_json().unwrap();
        assert!(!saved.contains("layout"), "{saved}");
    }

    #[test]
    fn positions_round_trip_exactly() {
        let document = Invention::from_json(PLACED).unwrap();
        let osc = document.layout.modules["osc"];
        assert_eq!((osc.x, osc.y), (120.5, -40.0));
        let again = Invention::from_json(&document.to_json().unwrap()).unwrap();
        assert_eq!(again.layout, document.layout);
    }

    #[test]
    fn saving_to_a_file_keeps_positions_and_adds_no_minimum_core() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("placed.json");
        let document = Invention::from_json(PLACED).unwrap();
        document.save_to_file(&path).unwrap();
        let loaded = Invention::from_file(path.to_str().unwrap()).unwrap();
        assert_eq!(loaded.layout, document.layout);
        assert_eq!(loaded.minimum_core_version, None);
    }

    #[test]
    fn a_document_without_a_layout_saves_to_a_file_byte_identically() {
        let dir = tempfile::tempdir().unwrap();
        let json = r#"{"modules":[{"id":"osc","type":"oscillator"}],"connections":[]}"#;
        let document = Invention::from_json(json).unwrap();
        let path = dir.path().join("plain.json");
        document.save_to_file(&path).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert_eq!(saved, format!("{}\n", document.to_json().unwrap()));
        assert!(!saved.contains("layout"), "{saved}");
    }

    #[test]
    fn a_position_with_an_unknown_field_is_refused() {
        let json = PLACED.replace(r#""y": -40"#, r#""y": -40, "z": 3"#);
        assert!(Invention::from_json(&json).is_err());
    }

    #[test]
    fn positions_save_in_id_order() {
        let saved = Invention::from_json(PLACED).unwrap().to_json().unwrap();
        let dac = saved.find("\"dac\": {\n").unwrap();
        let osc = saved.find("\"osc\": {\n").unwrap();
        assert!(dac < osc, "{saved}");
    }

    #[test]
    fn settling_drops_unknown_modules_and_non_finite_positions() {
        let mut document = Invention::from_json(PLACED).unwrap();
        let place = |x: f64| Position { x, y: 0.0 };
        document.layout.modules.insert("ghost".into(), place(1.0));
        document
            .layout
            .modules
            .insert("dac".into(), place(f64::NAN));
        let dropped = document.settle_layout();
        assert_eq!(dropped.len(), 2, "{dropped:?}");
        assert!(dropped.iter().any(|message| message.contains("'ghost'")));
        assert!(dropped.iter().any(|message| message.contains("not finite")));
        let kept: Vec<&str> = document.layout.modules.keys().map(String::as_str).collect();
        assert_eq!(kept, ["osc"]);
    }
}
