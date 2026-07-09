//! Piece metadata extraction for MusicXML conversion.
//!
//! Tempo is handled separately, in the main converter, because it is
//! positional: `<sound tempo>` marks compile into the score's tempo map at
//! their step positions rather than being read here as a single scalar.

use crate::invention::TimeSignature;

use super::child_text;
use crate::music::{key_signature_name, KeyMode};

pub(super) struct Metadata {
    pub(super) title: Option<String>,
    pub(super) composer: Option<String>,
    pub(super) key: Option<String>,
    pub(super) time_signature: Option<TimeSignature>,
}

/// Extracts piece metadata. Engraved `<credit>` text is preferred over the
/// `<work>`/`<identification>` fields because notation editors frequently
/// leave the latter as placeholders ("Untitled score") while the credits carry
/// the text actually printed on the page.
pub(super) fn extract_metadata(root: roxmltree::Node) -> Metadata {
    let credit = |kind: &str| -> Option<String> {
        root.children()
            .filter(|n| n.has_tag_name("credit"))
            .find(|n| child_text(*n, "credit-type") == Some(kind))
            .and_then(|n| {
                let mut words = String::new();
                for w in n.children().filter(|c| c.has_tag_name("credit-words")) {
                    if let Some(text) = w.text() {
                        if !words.is_empty() {
                            words.push(' ');
                        }
                        words.push_str(text.trim());
                    }
                }
                (!words.is_empty()).then_some(words)
            })
    };

    let title = credit("title")
        .or_else(|| {
            root.children()
                .find(|n| n.has_tag_name("movement-title"))
                .and_then(|n| n.text())
                .map(|t| t.trim().to_string())
        })
        .or_else(|| {
            root.children()
                .find(|n| n.has_tag_name("work"))
                .and_then(|w| child_text(w, "work-title"))
                .map(str::to_string)
        });
    let composer = credit("composer").or_else(|| {
        root.children()
            .find(|n| n.has_tag_name("identification"))
            .and_then(|id| {
                id.children()
                    .filter(|n| n.has_tag_name("creator"))
                    .find(|n| n.attribute("type") == Some("composer"))
                    .and_then(|n| n.text())
                    .map(|t| t.trim().to_string())
            })
    });

    let mut key = None;
    let mut time_signature = None;
    for node in root.descendants() {
        if key.is_none() && node.has_tag_name("key") {
            if let Some(fifths) = child_text(node, "fifths").and_then(|t| t.parse::<i32>().ok()) {
                // MusicXML <mode> is free-form; anything but "minor" (church
                // modes, "major", absent) is named from the major-key tonic.
                let mode = match child_text(node, "mode") {
                    Some("minor") => KeyMode::Minor,
                    _ => KeyMode::Major,
                };
                key = key_signature_name(fifths, mode);
            }
        }
        if time_signature.is_none() && node.has_tag_name("time") {
            if let (Some(Ok(beats)), Some(Ok(unit))) = (
                child_text(node, "beats").map(str::parse::<u32>),
                child_text(node, "beat-type").map(str::parse::<u32>),
            ) {
                if beats > 0 && unit > 0 {
                    time_signature = Some(TimeSignature {
                        beats_per_measure: beats,
                        beat_unit: unit,
                    });
                }
            }
        }
    }

    Metadata {
        title,
        composer,
        key,
        time_signature,
    }
}
