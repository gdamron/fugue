//! Reading the YAML frontmatter block at the top of a `SKILL.md`.
//!
//! Agent hosts discover a skill by the `name` and `description` in its
//! frontmatter, which they read as YAML. The block must parse as a YAML
//! mapping; a field is text only when YAML reads it as a string. Only the
//! top-level fields are kept, read from the parser's events.

use std::collections::BTreeMap;

use yaml_rust2::parser::{Event, Parser};
use yaml_rust2::scanner::TScalarStyle;
use yaml_rust2::Yaml;

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

/// The deepest collection nesting a frontmatter block may use. Skill
/// metadata is flat; the bound keeps a hostile file from costing more than a
/// counter.
const MAX_DEPTH: usize = 32;

/// A frontmatter field's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Field {
    /// Absent, null, or a string that is empty or only whitespace.
    Missing,
    /// A non-empty string, trimmed.
    Text(String),
    /// A value YAML reads as something other than a string: a number, a
    /// boolean, a list or a mapping.
    NotText,
}

/// A parsed frontmatter block: the top-level fields of a YAML mapping.
pub(crate) struct Frontmatter(BTreeMap<String, Field>);

impl Frontmatter {
    /// Read `frontmatter` as one YAML mapping (or nothing), keeping only its
    /// top-level fields. This walks the parser's events rather than building
    /// a document tree, so a small file cannot make it allocate copies of
    /// anchored values or recurse without bound; aliases are refused.
    pub(crate) fn parse(frontmatter: &str) -> Result<Self, String> {
        let mut parser = Parser::new_from_str(frontmatter);
        let mut fields = BTreeMap::new();
        let mut depth = 0usize;
        let mut documents = 0usize;
        // At depth 1, the key whose value comes next.
        let mut key: Option<String> = None;
        let mut record = |key: &mut Option<String>, field: Field| -> Result<(), String> {
            let Some(name) = key.take() else {
                return Ok(());
            };
            match fields.insert(name.clone(), field) {
                Some(_) => Err(format!("key `{name}` appears more than once")),
                None => Ok(()),
            }
        };
        loop {
            let (event, _) = parser.next_token().map_err(|err| err.to_string())?;
            match event {
                Event::StreamEnd => break,
                Event::DocumentStart => {
                    documents += 1;
                    if documents > 1 {
                        return Err("it holds more than one YAML document".to_string());
                    }
                }
                Event::Alias(_) => {
                    return Err("it uses a YAML alias (`*name`); write the value out".to_string())
                }
                Event::Scalar(value, style, _, _) => match depth {
                    0 if style == TScalarStyle::Plain
                        && matches!(Yaml::from_str(&value), Yaml::Null) => {}
                    0 => return Err(NOT_A_MAPPING.to_string()),
                    1 if key.is_none() => key = Some(value),
                    1 => record(&mut key, scalar_field(value, style))?,
                    _ => {}
                },
                Event::SequenceStart(..) | Event::MappingStart(..) => {
                    let mapping = matches!(event, Event::MappingStart(..));
                    match depth {
                        0 if !mapping => return Err(NOT_A_MAPPING.to_string()),
                        1 if key.is_none() => return Err("its keys must be plain text".to_string()),
                        1 => record(&mut key, Field::NotText)?,
                        _ => {}
                    }
                    depth += 1;
                    if depth > MAX_DEPTH {
                        return Err(format!("it nests deeper than {MAX_DEPTH} levels"));
                    }
                }
                Event::SequenceEnd | Event::MappingEnd => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        Ok(Frontmatter(fields))
    }

    /// The value of the top-level `key`.
    pub(crate) fn field(&self, key: &str) -> Field {
        self.0.get(key).cloned().unwrap_or(Field::Missing)
    }
}

const NOT_A_MAPPING: &str = "it is not a mapping of `key: value` pairs";

/// A scalar value as a field: plain scalars resolve like YAML's core schema
/// (`null`, numbers and booleans are not text), quoted and block scalars are
/// always strings.
fn scalar_field(value: String, style: TScalarStyle) -> Field {
    let text = if style == TScalarStyle::Plain {
        match Yaml::from_str(&value) {
            Yaml::Null => return Field::Missing,
            Yaml::String(text) => text,
            _ => return Field::NotText,
        }
    } else {
        value
    };
    match text.trim() {
        "" => Field::Missing,
        trimmed => Field::Text(trimmed.to_string()),
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
    fn anchors_cost_nothing_and_deep_nesting_is_refused() {
        let mut anchored = String::from("name: a\nextra: ");
        for level in 0..128 {
            anchored.push_str(&format!("&a{level} {{k: "));
        }
        anchored.push_str(&"x".repeat(1 << 16));
        anchored.push_str(&"}".repeat(128));
        anchored.push('\n');
        // Deeper than the bound, so refused; the anchors themselves are inert.
        assert!(Frontmatter::parse(&anchored).is_err());
        assert_eq!(
            field("name: a\nextra: &e {k: v}\n", "name"),
            Field::Text("a".to_string())
        );

        let deep = format!("name: a\nextra:\n  {}x\n", "- ".repeat(5000));
        let error = Frontmatter::parse(&deep).err().unwrap();
        assert!(error.contains("nests deeper"), "{error}");
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
