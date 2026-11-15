//! The `layout` member of one definition in a development file's text,
//! replaced, added or removed in place, leaving every other byte alone.

use crate::invention::layout::Layout;

/// One member of a JSON object in the text: its key, and where its key
/// starts and its value starts and ends (as byte offsets).
struct Member {
    key: String,
    start: usize,
    value: usize,
    end: usize,
}

/// `text` with `layout` as the layout of the definition `within` names
/// (each an inline declaration in the one before it), or `None` when the
/// text is not a document holding that definition.
pub(super) fn with_layout(text: &str, within: &[String], layout: &Layout) -> Option<String> {
    let bytes = text.as_bytes();
    let mut object = skip_space(bytes, 0);
    for name in within {
        let own = members(bytes, object)?.0;
        let developments = own.iter().find(|m| m.key == "developments")?;
        let declaration = elements(bytes, developments.value)?
            .into_iter()
            .find(|&at| {
                let found = members(bytes, at).map(|(members, _)| members);
                let named = found.iter().flatten().find(|m| m.key == "name");
                named.and_then(|m| serde_json::from_str::<String>(&text[m.value..m.end]).ok())
                    == Some(name.clone())
            })?;
        let declared = members(bytes, declaration)?.0;
        object = declared.iter().find(|m| m.key == "definition")?.value;
    }
    let own = members(bytes, object)?.0;
    let found = own.iter().position(|m| m.key == "layout");
    let mut out = String::with_capacity(text.len() + 64);
    match (found, layout.is_empty()) {
        (None, true) => return Some(text.to_string()),
        (Some(at), false) => {
            let member = &own[at];
            let indent = indent_at(text, member.start);
            out.push_str(&text[..member.value]);
            out.push_str(&json(layout, indent)?);
            out.push_str(&text[member.end..]);
        }
        (None, false) => {
            let last = own.last()?;
            let indent = indent_at(text, last.start);
            out.push_str(&text[..last.end]);
            out.push_str(&format!(",\n{indent}\"layout\": {}", json(layout, indent)?));
            out.push_str(&text[last.end..]);
        }
        (Some(at), true) => {
            // The member goes, with the comma that joined it to its neighbor.
            let (from, to) = match at {
                0 => match own.get(1) {
                    Some(next) => (own[0].start, next.start),
                    None => (own[0].start, own[0].end),
                },
                _ => (own[at - 1].end, own[at].end),
            };
            out.push_str(&text[..from]);
            out.push_str(&text[to..]);
        }
    }
    Some(out)
}

/// `layout` as pretty JSON, its later lines indented by `indent`.
fn json(layout: &Layout, indent: &str) -> Option<String> {
    let pretty = serde_json::to_string_pretty(layout).ok()?;
    Some(pretty.replace('\n', &format!("\n{indent}")))
}

/// The spaces or tabs that start the line `at` is on, up to `at`.
fn indent_at(text: &str, at: usize) -> &str {
    let line = text[..at].rfind('\n').map_or(0, |n| n + 1);
    let lead = &text[line..at];
    &lead[..lead.len() - lead.trim_start_matches([' ', '\t']).len()]
}

fn skip_space(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() && bytes[at].is_ascii_whitespace() {
        at += 1;
    }
    at
}

/// The end of the JSON value starting at `at` (after any space).
fn value_end(bytes: &[u8], at: usize) -> Option<usize> {
    let mut at = skip_space(bytes, at);
    match *bytes.get(at)? {
        b'"' => string_end(bytes, at),
        open @ (b'{' | b'[') => {
            let close = if open == b'{' { b'}' } else { b']' };
            at += 1;
            loop {
                at = skip_space(bytes, at);
                match *bytes.get(at)? {
                    byte if byte == close => return Some(at + 1),
                    b',' | b':' => at += 1,
                    _ => at = value_end(bytes, at)?,
                }
            }
        }
        _ => {
            while at < bytes.len()
                && !matches!(bytes[at], b',' | b'}' | b']')
                && !bytes[at].is_ascii_whitespace()
            {
                at += 1;
            }
            Some(at)
        }
    }
}

/// The end of the string starting at `at`, its closing quote included.
fn string_end(bytes: &[u8], mut at: usize) -> Option<usize> {
    at += 1;
    loop {
        match *bytes.get(at)? {
            b'\\' => at += 2,
            b'"' => return Some(at + 1),
            _ => at += 1,
        }
    }
}

/// The members of the object starting at `open`, and where it closes.
fn members(bytes: &[u8], open: usize) -> Option<(Vec<Member>, usize)> {
    if bytes.get(open) != Some(&b'{') {
        return None;
    }
    let mut found = Vec::new();
    let mut at = open + 1;
    loop {
        at = skip_space(bytes, at);
        match *bytes.get(at)? {
            b'}' => return Some((found, at)),
            b',' => at += 1,
            b'"' => {
                let start = at;
                let key_end = string_end(bytes, at)?;
                let key = std::str::from_utf8(&bytes[start..key_end]).ok()?;
                let key = serde_json::from_str::<String>(key).ok()?;
                at = skip_space(bytes, key_end);
                if bytes.get(at) != Some(&b':') {
                    return None;
                }
                let value = skip_space(bytes, at + 1);
                let end = value_end(bytes, value)?;
                found.push(Member {
                    key,
                    start,
                    value,
                    end,
                });
                at = end;
            }
            _ => return None,
        }
    }
}

/// Where each element of the array at `open` starts.
fn elements(bytes: &[u8], open: usize) -> Option<Vec<usize>> {
    if bytes.get(open) != Some(&b'[') {
        return None;
    }
    let mut found = Vec::new();
    let mut at = open + 1;
    loop {
        at = skip_space(bytes, at);
        match *bytes.get(at)? {
            b']' => return Some(found),
            b',' => at += 1,
            _ => {
                found.push(at);
                at = value_end(bytes, at)?;
            }
        }
    }
}
