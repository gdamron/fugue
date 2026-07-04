//! Key signatures on the circle of fifths.

/// Major or minor mode of a key signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyMode {
    #[default]
    Major,
    Minor,
}

/// Names a key from its circle-of-fifths position (flats negative, sharps
/// positive, as in MusicXML's `<fifths>`): `-4` → "Ab major" / "F minor".
/// Returns `None` outside the notated range of -7..=7.
pub fn key_signature_name(fifths: i32, mode: KeyMode) -> Option<String> {
    const MAJORS: [&str; 15] = [
        "Cb", "Gb", "Db", "Ab", "Eb", "Bb", "F", "C", "G", "D", "A", "E", "B", "F#", "C#",
    ];
    const MINORS: [&str; 15] = [
        "Ab", "Eb", "Bb", "F", "C", "G", "D", "A", "E", "B", "F#", "C#", "G#", "D#", "A#",
    ];
    let index = usize::try_from(fifths + 7).ok().filter(|&i| i < 15)?;
    Some(match mode {
        KeyMode::Major => format!("{} major", MAJORS[index]),
        KeyMode::Minor => format!("{} minor", MINORS[index]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_keys_on_the_circle_of_fifths() {
        assert_eq!(
            key_signature_name(0, KeyMode::Major).as_deref(),
            Some("C major")
        );
        assert_eq!(
            key_signature_name(-4, KeyMode::Major).as_deref(),
            Some("Ab major")
        );
        assert_eq!(
            key_signature_name(-4, KeyMode::Minor).as_deref(),
            Some("F minor")
        );
        assert_eq!(
            key_signature_name(7, KeyMode::Major).as_deref(),
            Some("C# major")
        );
        assert_eq!(key_signature_name(8, KeyMode::Major), None);
        assert_eq!(key_signature_name(-8, KeyMode::Minor), None);
    }
}
