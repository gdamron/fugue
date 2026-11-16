//! Reading the YAML frontmatter block at the top of a `SKILL.md`.
//!
//! Agent hosts discover a skill by the `name` and `description` in its
//! frontmatter. This reads the block's top-level scalar keys, which is all a
//! skill check needs, without a YAML dependency: plain, single- and
//! double-quoted scalars, block scalars (`|`, `>`), and plain scalars
//! continued on indented lines. Nested values are kept as their raw text.

use std::collections::BTreeMap;

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

/// The frontmatter's top-level keys and their scalar values, trimmed.
pub(crate) fn top_level_values(frontmatter: &str) -> Result<BTreeMap<String, String>, String> {
    let mut values = BTreeMap::new();
    let lines: Vec<&str> = frontmatter.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            // Continuation of a value already consumed below; a stray one is
            // nested data this check does not read.
            continue;
        }
        let Some((key, raw)) = line.split_once(':') else {
            return Err(format!("line `{line}` is not a `key: value` pair"));
        };
        let key = unquote(key.trim()).trim().to_string();
        let raw = raw.trim();
        // Indented lines that follow belong to this key.
        let mut continued = Vec::new();
        while index < lines.len()
            && (lines[index].trim().is_empty() || lines[index].starts_with([' ', '\t']))
        {
            continued.push(lines[index].trim());
            index += 1;
        }
        let value = if raw.starts_with(['|', '>']) {
            continued.join("\n").trim().to_string()
        } else {
            let first = scalar(raw)?;
            let tail = continued.join(" ");
            format!("{first} {tail}").trim().to_string()
        };
        if values.insert(key.clone(), value).is_some() {
            return Err(format!("key `{key}` appears more than once"));
        }
    }
    Ok(values)
}

/// A one-line scalar: quotes removed, a trailing comment dropped from a plain
/// scalar.
fn scalar(raw: &str) -> Result<String, String> {
    if let Some(inner) = raw.strip_prefix('"') {
        let end = inner
            .rfind('"')
            .ok_or_else(|| format!("value `{raw}` has an unclosed double quote"))?;
        return Ok(inner[..end].replace("\\\"", "\""));
    }
    if let Some(inner) = raw.strip_prefix('\'') {
        let end = inner
            .rfind('\'')
            .ok_or_else(|| format!("value `{raw}` has an unclosed single quote"))?;
        return Ok(inner[..end].replace("''", "'"));
    }
    let plain = match raw.find(" #") {
        Some(comment) => &raw[..comment],
        None if raw.starts_with('#') => "",
        None => raw,
    };
    Ok(plain.trim().to_string())
}

fn unquote(key: &str) -> &str {
    key.strip_prefix('"')
        .and_then(|k| k.strip_suffix('"'))
        .or_else(|| key.strip_prefix('\'').and_then(|k| k.strip_suffix('\'')))
        .unwrap_or(key)
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

    #[test]
    fn reads_scalar_forms() {
        let values = top_level_values(
            "name: \"quoted-name\"\n\
             plain: text # comment\n\
             single: 'it''s'\n\
             folded: >\n  first line\n  second line\n\
             continued: starts\n  and continues\n\
             empty:\n\
             nested:\n  inner: 1\n",
        )
        .unwrap();
        assert_eq!(values["name"], "quoted-name");
        assert_eq!(values["plain"], "text");
        assert_eq!(values["single"], "it's");
        assert_eq!(values["folded"], "first line\nsecond line");
        assert_eq!(values["continued"], "starts and continues");
        assert_eq!(values["empty"], "");
        assert_eq!(values["nested"], "inner: 1");
    }

    #[test]
    fn refuses_duplicates_and_non_pairs() {
        assert!(top_level_values("name: a\nname: b\n").is_err());
        assert!(top_level_values("just text\n").is_err());
        assert!(top_level_values("name: \"open\n").is_err());
    }
}
