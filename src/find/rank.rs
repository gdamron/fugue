//! Lexical ranking: words, their weights, and the nearest words when a
//! query matches nothing.

use std::collections::{BTreeMap, HashMap};

use super::{synonyms, Entry};

/// How much a word counts by where an entry holds it: its name most, then
/// what it is, its key words, and the prose of when to use it.
const NAME_WEIGHT: f64 = 3.0;
const WHAT_WEIGHT: f64 = 2.0;
const TERMS_WEIGHT: f64 = 1.5;
const WHEN_WEIGHT: f64 = 1.0;

/// How much a match counts by how it matched the query's word.
const EXACT: f64 = 1.0;
const SYNONYM: f64 = 0.7;
const PREFIX: f64 = 0.5;

/// What each further word a query word reaches in an entry adds, beyond the
/// best one: a control that holds both "echo" (its module) and "feedback"
/// (what echo means) ranks ahead of one that holds only the module, and
/// a clock's bpm ahead of a delay's for "slow".
const FURTHER_MATCH: f64 = 0.25;

/// A query that names an entry outright ranks it far ahead.
const NAME_MATCH_BONUS: f64 = 2.5;

/// The shortest query word a longer word may start with and still match.
const MIN_PREFIX: usize = 3;

/// Words that carry no meaning for a search: "make it brighter" searches
/// for "brighter".
const STOP_WORDS: &[&str] = &[
    "a", "about", "add", "all", "an", "and", "any", "are", "at", "be", "bit", "by", "can", "could",
    "do", "does", "for", "from", "give", "how", "i", "in", "into", "is", "it", "its", "let",
    "little", "make", "me", "more", "much", "my", "need", "of", "on", "or", "our", "please",
    "should", "so", "some", "that", "the", "their", "them", "there", "these", "this", "those",
    "to", "up", "us", "use", "want", "way", "we", "what", "when", "where", "which", "with",
    "would", "you", "your",
];

#[cfg(test)]
pub(super) fn stop_words() -> &'static [&'static str] {
    STOP_WORDS
}

/// One entry that matched, and how well.
#[derive(Debug, Clone, Copy)]
pub(super) struct Hit {
    pub(super) entry: usize,
    /// How many of the query's words the entry matches.
    pub(super) matched: usize,
    pub(super) score: f64,
}

/// The entries that match `words`, best first: those matching more of the
/// query's words ahead, then by score. Ties fall by kind, then
/// name, then the order entries were added, so the same index and query
/// always rank the same.
pub(super) fn rank(entries: &[Entry], words: &str) -> Vec<Hit> {
    let query = query_words(words);
    if query.is_empty() {
        return Vec::new();
    }
    let indexed: Vec<HashMap<String, f64>> = entries.iter().map(entry_words).collect();
    let mut frequency: HashMap<&str, usize> = HashMap::new();
    for words in &indexed {
        for word in words.keys() {
            *frequency.entry(word).or_default() += 1;
        }
    }
    let count = entries.len() as f64;
    let rarity = |word: &str| (1.0 + count / frequency.get(word).copied().unwrap_or(1) as f64).ln();
    let expansions: Vec<Vec<(String, f64)>> = query.iter().map(|word| expand(word)).collect();
    let named = all_words(words).join(" ");

    let mut hits: Vec<Hit> = Vec::new();
    for (at, words) in indexed.iter().enumerate() {
        let mut total = 0.0;
        let mut matched = 0;
        for candidates in &expansions {
            let best = best_match(words, candidates, &rarity);
            if best > 0.0 {
                total += best;
                matched += 1;
            }
        }
        if matched == 0 {
            continue;
        }
        let mut score = total * matched as f64 / query.len() as f64;
        if all_words(&entries[at].card.name).join(" ") == named {
            score *= NAME_MATCH_BONUS;
        }
        hits.push(Hit {
            entry: at,
            matched,
            score,
        });
    }
    hits.sort_by(|a, b| {
        let (ca, cb) = (&entries[a.entry].card, &entries[b.entry].card);
        (b.matched.cmp(&a.matched))
            .then(b.score.total_cmp(&a.score))
            .then(ca.kind.cmp(&cb.kind))
            .then(ca.name.cmp(&cb.name))
            .then(a.entry.cmp(&b.entry))
    });
    hits
}

/// How well one query word's `candidates` match an entry's `words`: the
/// best weight any of them reaches, plus a little for each further one the
/// entry holds.
fn best_match(
    words: &HashMap<String, f64>,
    candidates: &[(String, f64)],
    rarity: &impl Fn(&str) -> f64,
) -> f64 {
    let mut best: f64 = 0.0;
    let mut held = 0.0;
    for (candidate, weight) in candidates {
        if let Some(field) = words.get(candidate) {
            let score = field * weight * rarity(candidate);
            held += score;
            best = best.max(score);
        }
        if *weight == EXACT && candidate.chars().count() >= MIN_PREFIX {
            for (word, field) in words {
                if word.len() > candidate.len() && word.starts_with(candidate.as_str()) {
                    best = best.max(field * PREFIX * rarity(word));
                }
            }
        }
    }
    best + FURTHER_MATCH * (held - best).max(0.0)
}

/// A query word and the words the synonym table widens it to.
fn expand(word: &str) -> Vec<(String, f64)> {
    let mut candidates = vec![(word.to_string(), EXACT)];
    for synonym in synonyms::of(word) {
        if !candidates.iter().any(|(known, _)| known == synonym) {
            candidates.push((synonym.to_string(), SYNONYM));
        }
    }
    candidates
}

/// Each word an entry holds, with the weight of the most telling field
/// that holds it.
fn entry_words(entry: &Entry) -> HashMap<String, f64> {
    let mut words = HashMap::new();
    let mut add = |text: &str, weight: f64| {
        // A name keeps every word, so a tool named "what" is found by it.
        let words_of = if weight == NAME_WEIGHT {
            all_words(text)
        } else {
            meaningful(text)
        };
        for word in words_of {
            let known = words.entry(word).or_insert(0.0);
            if weight > *known {
                *known = weight;
            }
        }
    };
    add(&entry.card.name, NAME_WEIGHT);
    add(&entry.card.what, WHAT_WEIGHT);
    for term in &entry.terms {
        add(term, TERMS_WEIGHT);
    }
    add(&entry.card.when, WHEN_WEIGHT);
    for text in &entry.text {
        add(text, WHEN_WEIGHT);
    }
    words
}

/// The words a query searches for: its meaningful words, or every word
/// when all of them are stop words.
fn query_words(text: &str) -> Vec<String> {
    let mut words = meaningful(text);
    if words.is_empty() {
        words = all_words(text);
    }
    words.dedup();
    words
}

/// The words of `text` that carry meaning, normalized.
pub(super) fn meaningful(text: &str) -> Vec<String> {
    (all_words(text).into_iter())
        .filter(|word| STOP_WORDS.binary_search(&word.as_str()).is_err())
        .collect()
}

/// Every word of `text`, lowercase and singular. Words split at anything
/// but a letter or digit, and where a lowercase letter meets a capital
/// (`filterCutoff` is "filter cutoff").
pub(super) fn all_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for ch in text.chars() {
        if !ch.is_alphanumeric() {
            push_word(&mut words, &mut current);
            previous_lower = false;
            continue;
        }
        if ch.is_uppercase() && previous_lower {
            push_word(&mut words, &mut current);
        }
        previous_lower = ch.is_lowercase() || ch.is_ascii_digit();
        current.extend(ch.to_lowercase());
    }
    push_word(&mut words, &mut current);
    words
}

fn push_word(words: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        words.push(singular(current));
        current.clear();
    }
}

/// `word` in the singular, by its ending alone: "notes" is "note",
/// "echoes" "echo", "melodies" "melody". Applied alike to the index and the
/// query, so a wrong guess still matches itself.
pub(super) fn singular(word: &str) -> String {
    let count = word.chars().count();
    if count > 4 && word.ends_with("ies") {
        return format!("{}y", &word[..word.len() - 3]);
    }
    if count > 4
        && word.ends_with("es")
        && ["s", "x", "z", "ch", "sh", "o"]
            .iter()
            .any(|end| word[..word.len() - 2].ends_with(end))
    {
        return word[..word.len() - 2].to_string();
    }
    if count > 3 && word.ends_with('s') && !word.ends_with("ss") && !word.ends_with("us") {
        return word[..word.len() - 1].to_string();
    }
    word.to_string()
}

/// How many words a query that is spelled like nothing still gets.
const FALLBACK_TERMS: usize = 3;
/// The longest word offered as a nearest term.
const MAX_TERM_CHARS: usize = 32;

/// The words the index holds that are spelled closest to the query's,
/// for a query that matched nothing: at most five, nearest first, or the
/// closest three when none is spelled close.
pub(super) fn nearest_terms(entries: &[Entry], words: &str) -> Vec<String> {
    let query = query_words(words);
    let mut frequency: BTreeMap<String, usize> = BTreeMap::new();
    for entry in entries {
        let card = &entry.card;
        let texts = [card.name.as_str(), card.what.as_str()];
        for text in texts
            .into_iter()
            .chain(entry.terms.iter().map(String::as_str))
        {
            for word in meaningful(text) {
                if word.chars().count() >= MIN_PREFIX && !word.chars().all(|c| c.is_ascii_digit()) {
                    *frequency.entry(word).or_default() += 1;
                }
            }
        }
    }
    let long: Vec<&String> = (query.iter())
        .filter(|asked| asked.chars().count() >= MIN_PREFIX)
        .collect();
    let asked = if long.is_empty() {
        query.iter().collect()
    } else {
        long
    };
    let mut near: Vec<(usize, std::cmp::Reverse<usize>, String)> = Vec::new();
    let mut all_near = Vec::new();
    for (word, count) in frequency {
        let length = word.chars().count();
        if length > MAX_TERM_CHARS {
            continue;
        }
        let Some(closest) = asked.iter().map(|asked| distance(asked, &word)).min() else {
            continue;
        };
        let entry = (closest, std::cmp::Reverse(count), word);
        if closest <= (length / 3).max(1) {
            near.push(entry.clone());
        }
        all_near.push(entry);
    }
    near.sort();
    if near.is_empty() {
        // Nothing is spelled close: the closest few still say where to look.
        let mut closest = all_near;
        closest.sort();
        return (closest.into_iter().take(FALLBACK_TERMS))
            .map(|(_, _, word)| word)
            .collect();
    }
    near.into_iter().take(5).map(|(_, _, word)| word).collect()
}

/// The edit distance between two words, in characters.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (previous + usize::from(ca != *cb))
                .min(above + 1)
                .min(row[j] + 1);
            previous = above;
        }
    }
    row[b.len()]
}
