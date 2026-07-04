//! Piece metadata and grid naming for MusicXML conversion.

use std::fmt::Write as _;

use crate::invention::TimeSignature;

use super::{child_text, ConvertReport};
use crate::invention::score::import::rational::Rat;

pub(super) struct Metadata {
    pub(super) title: Option<String>,
    pub(super) composer: Option<String>,
    pub(super) key: Option<String>,
    pub(super) tempo: Option<f32>,
    pub(super) time_signature: Option<TimeSignature>,
}

/// Extracts piece metadata. Engraved `<credit>` text is preferred over the
/// `<work>`/`<identification>` fields because notation editors frequently
/// leave the latter as placeholders ("Untitled score") while the credits carry
/// the text actually printed on the page.
pub(super) fn extract_metadata(root: roxmltree::Node, report: &mut ConvertReport) -> Metadata {
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
    let mut tempo = None;
    let mut tempo_changes = Vec::new();
    for node in root.descendants() {
        if key.is_none() && node.has_tag_name("key") {
            if let Some(fifths) = child_text(node, "fifths").and_then(|t| t.parse::<i32>().ok()) {
                key = key_name(fifths, child_text(node, "mode"));
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
        if node.has_tag_name("sound") {
            if let Some(bpm) = node.attribute("tempo").and_then(|t| t.parse::<f32>().ok()) {
                if tempo.is_none() {
                    tempo = Some(bpm);
                } else if Some(bpm) != tempo && !tempo_changes.contains(&bpm) {
                    tempo_changes.push(bpm);
                }
            }
        }
    }
    if !tempo_changes.is_empty() {
        let mut list = String::new();
        for (i, bpm) in tempo_changes.iter().enumerate() {
            if i > 0 {
                let _ = write!(list, ", ");
            }
            let _ = write!(list, "{}", bpm);
        }
        report.warnings.push(format!(
            "tempo changes ({} bpm) are not representable in fugue.score.v1; kept the initial tempo",
            list
        ));
    }

    Metadata {
        title,
        composer,
        key,
        tempo,
        time_signature,
    }
}

/// Human-readable grid name, e.g. `16th_note` or `8th_triplet`, from the grid
/// expressed as a fraction of a whole note.
pub(super) fn grid_name(grid: Rat) -> String {
    let whole = Rat::new(grid.num, grid.den * 4);
    if whole.num == 1 {
        match whole.den {
            1 => return "whole_note".to_string(),
            2 => return "half_note".to_string(),
            4 => return "quarter_note".to_string(),
            8 => return "8th_note".to_string(),
            16 => return "16th_note".to_string(),
            32 => return "32nd_note".to_string(),
            64 => return "64th_note".to_string(),
            128 => return "128th_note".to_string(),
            3 => return "half_triplet".to_string(),
            6 => return "quarter_triplet".to_string(),
            12 => return "8th_triplet".to_string(),
            24 => return "16th_triplet".to_string(),
            48 => return "32nd_triplet".to_string(),
            _ => {}
        }
    }
    format!("{}/{}_whole_note", whole.num, whole.den)
}

/// Key name from circle-of-fifths position, e.g. `-4` → "Ab major"/"F minor".
fn key_name(fifths: i32, mode: Option<&str>) -> Option<String> {
    const MAJORS: [&str; 15] = [
        "Cb", "Gb", "Db", "Ab", "Eb", "Bb", "F", "C", "G", "D", "A", "E", "B", "F#", "C#",
    ];
    const MINORS: [&str; 15] = [
        "Ab", "Eb", "Bb", "F", "C", "G", "D", "A", "E", "B", "F#", "C#", "G#", "D#", "A#",
    ];
    let index = usize::try_from(fifths + 7).ok().filter(|&i| i < 15)?;
    Some(match mode {
        Some("minor") => format!("{} minor", MINORS[index]),
        _ => format!("{} major", MAJORS[index]),
    })
}
