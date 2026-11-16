//! Reading the YAML frontmatter block at the top of a `SKILL.md`.
//!
//! Agent hosts discover a skill by the `name` and `description` in its
//! frontmatter, which they read as YAML. The block must parse as a YAML
//! mapping; a field is text only when YAML reads it as a string.

use yaml_rust2::parser::{Event, Parser};
use yaml_rust2::{Yaml, YamlLoader};

/// A `SKILL.md` split at its frontmatter block.
pub(crate) struct Split<'a> {
    /// The lines between the opening and closing fences.
    pub(crate) frontmatter: &'a str,
    /// Everything after the closing fence.
    pub(crate) body: &'a str,
}

/// Split `text` at a frontmatter block that opens on its first line and
/// closes with a `---` (or `...`) line. `None` when there is no such block.
pub(crate) fn split(text: &str) -> Option<Split<'_>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (first, rest) = text.split_once('\n')?;
    if first.trim_end() != "---" {
        return None;
    }
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if trimmed == "---" || trimmed == "..." {
            return Some(Split {
                frontmatter: &rest[..offset],
                body: &rest[offset + line.len()..],
            });
        }
        offset += line.len();
    }
    None
}

/// Refuse YAML aliases (`*name`) before building the tree: the loader copies
/// each alias's target, so nested aliases grow exponentially from a small
/// file. Skill metadata never needs them.
fn refuse_aliases(frontmatter: &str) -> Result<(), String> {
    let mut parser = Parser::new_from_str(frontmatter);
    loop {
        match parser.next_token().map_err(|err| err.to_string())? {
            (Event::StreamEnd, _) => return Ok(()),
            (Event::Alias(_), _) => {
                return Err("it uses a YAML alias (`*name`); write the value out".to_string())
            }
            _ => {}
        }
    }
}

/// A frontmatter field's value.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Field {
    /// Absent, null, or a string that is empty or only whitespace.
    Missing,
    /// A non-empty string, trimmed.
    Text(String),
    /// A value YAML reads as something other than a string: a number, a
    /// boolean, a list or a mapping.
    NotText,
}

/// A parsed frontmatter block: a YAML mapping.
pub(crate) struct Frontmatter(Yaml);

impl Frontmatter {
    /// Parse `frontmatter` as YAML; it must be a mapping (or empty) with no
    /// aliases.
    pub(crate) fn parse(frontmatter: &str) -> Result<Self, String> {
        refuse_aliases(frontmatter)?;
        let mut documents =
            YamlLoader::load_from_str(frontmatter).map_err(|err| err.to_string())?;
        match documents.len() {
            0 => Ok(Frontmatter(Yaml::Null)),
            1 => match documents.remove(0) {
                mapping @ (Yaml::Hash(_) | Yaml::Null) => Ok(Frontmatter(mapping)),
                _ => Err("it is not a mapping of `key: value` pairs".to_string()),
            },
            _ => Err("it holds more than one YAML document".to_string()),
        }
    }

    /// The value of the top-level `key`.
    pub(crate) fn field(&self, key: &str) -> Field {
        let Yaml::Hash(mapping) = &self.0 else {
            return Field::Missing;
        };
        match mapping.get(&Yaml::String(key.to_string())) {
            None | Some(Yaml::Null) => Field::Missing,
            Some(Yaml::String(text)) if text.trim().is_empty() => Field::Missing,
            Some(Yaml::String(text)) => Field::Text(text.trim().to_string()),
            Some(_) => Field::NotText,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_at_the_closing_fence() {
        let parts = split("---\nname: a\n---\n# Body\n").unwrap();
        assert_eq!(parts.frontmatter, "name: a\n");
        assert_eq!(parts.body, "# Body\n");
        assert!(split("# Body\n").is_none());
        assert!(split("---\nname: a\n").is_none(), "unclosed");
        assert!(split("\u{feff}---\r\nname: a\r\n---\r\n").is_some());
    }

    fn field(frontmatter: &str, key: &str) -> Field {
        Frontmatter::parse(frontmatter).unwrap().field(key)
    }

    #[test]
    fn reads_string_forms() {
        let text = |value: &str| Field::Text(value.to_string());
        assert_eq!(field("name: \"quoted\"\n", "name"), text("quoted"));
        assert_eq!(field("name: plain # comment\n", "name"), text("plain"));
        assert_eq!(field("d: 'it''s'\n", "d"), text("it's"));
        assert_eq!(
            field("d: >\n  first\n  second\n", "d"),
            text("first second")
        );
        assert_eq!(
            field("d: |\n  first\n  second\n", "d"),
            text("first\nsecond")
        );
        assert_eq!(field("d: \"multi\n  line\"\n", "d"), text("multi line"));
        assert_eq!(
            field("d: starts\n  continues\n", "d"),
            text("starts continues")
        );
    }

    #[test]
    fn nulls_blanks_and_comments_are_missing() {
        for frontmatter in [
            "other: x\n",
            "d:\n",
            "d: null\n",
            "d: ~\n",
            "d: \"  \"\n",
            "d:\n  # only a comment\n",
            "",
        ] {
            assert_eq!(field(frontmatter, "d"), Field::Missing, "{frontmatter:?}");
        }
    }

    #[test]
    fn other_types_are_not_text() {
        for frontmatter in ["d: 12\n", "d: true\n", "d: [a]\n", "d:\n  inner: 1\n"] {
            assert_eq!(field(frontmatter, "d"), Field::NotText, "{frontmatter:?}");
        }
    }

    #[test]
    fn refuses_aliases_before_expanding_them() {
        let mut frontmatter = String::from("name: a\ndescription: d\nl0: &l0 [x]\n");
        for level in 1..40 {
            let previous = level - 1;
            frontmatter.push_str(&format!(
                "l{level}: &l{level} [{}]\n",
                vec![format!("*l{previous}"); 10].join(", ")
            ));
        }
        let error = Frontmatter::parse(&frontmatter).err().unwrap();
        assert!(error.contains("alias"), "{error}");
        assert!(
            Frontmatter::parse("name: &n a\n").is_ok(),
            "an anchor alone is fine"
        );
    }

    #[test]
    fn refuses_malformed_yaml_and_non_mappings() {
        for frontmatter in [
            "name: a\ndescription: [\n",
            "just text\n",
            "- a\n- b\n",
            "name: \"open\n",
        ] {
            assert!(Frontmatter::parse(frontmatter).is_err(), "{frontmatter:?}");
        }
    }
}
