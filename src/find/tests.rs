use serde_json::json;

use super::rank::{all_words, singular};
use super::*;

fn card(kind: FindKind, name: &str, what: &str, when: &str) -> Entry {
    Entry::new(Card {
        kind,
        name: name.into(),
        what: what.into(),
        when: when.into(),
        how: json!({}),
    })
}

/// A small index: two module types, a tool, a view and a control.
fn index() -> Index {
    let mut index = Index::new();
    index.add(
        card(
            FindKind::ModuleType,
            "filter",
            "Filter module",
            "A filter module.",
        )
        .with_terms(["cutoff", "resonance", "lowpass"]),
    );
    index.add(
        card(
            FindKind::ModuleType,
            "delay",
            "Delay module",
            "A delay module.",
        )
        .with_terms(["time", "feedback"]),
    );
    index.add(card(
        FindKind::ModuleType,
        "reverb",
        "Reverb module",
        "A reverb module.",
    ));
    index.add(card(
        FindKind::Tool,
        "get_notes",
        "Get Notes",
        "Read a sequencer's notes.",
    ));
    index.add(card(
        FindKind::View,
        "piano-roll",
        "Piano roll",
        "One module's notes against musical time.",
    ));
    index.add(
        card(
            FindKind::Control,
            "lead_filter.cutoff",
            "Cutoff of lead_filter (filter)",
            "Cutoff frequency in Hz.",
        )
        .with_terms(["cutoff", "lead_filter", "filter"]),
    );
    index.set_piece_loaded(true);
    index
}

fn names(answer: &FindAnswer) -> Vec<&str> {
    answer.cards.iter().map(|card| card.name.as_str()).collect()
}

#[test]
fn words_are_split_lowercased_and_singular() {
    assert_eq!(all_words("apply_edits"), ["apply", "edit"]);
    assert_eq!(
        all_words("filterCutoff level.0"),
        ["filter", "cutoff", "level", "0"]
    );
    assert_eq!(singular("notes"), "note");
    assert_eq!(singular("echoes"), "echo");
    assert_eq!(singular("melodies"), "melody");
    assert_eq!(singular("bus"), "bus");
    assert_eq!(singular("glass"), "glass");
}

#[test]
fn stop_words_are_sorted_for_lookup() {
    let words = rank::stop_words();
    assert!(words.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn every_synonym_names_words_a_search_can_reach() {
    for (heard, meant) in synonyms::SYNONYMS {
        assert!(!heard.is_empty() && !meant.is_empty());
        for word in heard.iter().chain(meant.iter()) {
            assert!(!all_words(word).is_empty(), "{word} has no words");
        }
    }
}

#[test]
fn a_musical_word_reaches_what_fugue_calls_it() {
    let answer = index().find(&FindQuery::words("make it brighter")).unwrap();
    assert_eq!(names(&answer)[..2], ["lead_filter.cutoff", "filter"]);
    let answer = index().find(&FindQuery::words("echo")).unwrap();
    assert_eq!(names(&answer), ["delay"]);
}

#[test]
fn naming_a_thing_ranks_it_first() {
    let answer = index().find(&FindQuery::words("filter")).unwrap();
    assert_eq!(names(&answer)[0], "filter");
    let answer = index().find(&FindQuery::words("get_notes")).unwrap();
    assert_eq!(names(&answer)[0], "get_notes");
}

#[test]
fn the_same_index_and_query_answer_the_same() {
    let query = FindQuery::words("notes");
    let first = index().find(&query).unwrap();
    assert_eq!(first, index().find(&query).unwrap());
    assert_eq!(names(&first), ["get_notes", "piano-roll"]);
}

#[test]
fn a_kind_keeps_only_its_cards() {
    let query = FindQuery {
        kind: Some(FindKind::ModuleType),
        ..FindQuery::words("cutoff")
    };
    let answer = index().find(&query).unwrap();
    assert_eq!(names(&answer), ["filter"]);
}

#[test]
fn a_limit_counts_what_it_leaves_out() {
    let query = FindQuery {
        limit: Some(1),
        ..FindQuery::words("module")
    };
    let answer = index().find(&query).unwrap();
    assert_eq!(answer.cards.len(), 1);
    // Three module types and the view whose description says "module".
    assert_eq!(answer.more, 3);
    assert!(answer
        .notes
        .iter()
        .any(|note| note.starts_with("3 more matched")));
}

#[test]
fn the_byte_cap_stops_the_list_and_says_so() {
    let mut index = Index::new();
    let big = "x".repeat(MAX_FIND_BYTES / 3);
    for at in 0..5 {
        index.add(card(FindKind::Tool, &format!("big_{at}"), "Big", &big));
    }
    index.set_piece_loaded(true);
    let answer = index.find(&FindQuery::words("big")).unwrap();
    assert_eq!(answer.cards.len(), 2);
    assert_eq!(answer.more, 3);
    assert!(serde_json::to_vec(&answer).unwrap().len() <= MAX_FIND_BYTES);
    assert!(answer.notes.iter().any(|note| note.contains("stops at")));
}

#[test]
fn nothing_matched_is_an_answer_with_where_to_look() {
    let answer = index().find(&FindQuery::words("filtr")).unwrap();
    assert!(answer.cards.is_empty());
    assert!(answer.notes.contains(&"Nothing matched.".to_string()));
    assert_eq!(answer.nearest.unwrap().terms, ["filter"]);

    let query = FindQuery {
        kind: Some(FindKind::Example),
        ..FindQuery::words("reverb")
    };
    let nearest = index().find(&query).unwrap().nearest.unwrap();
    assert_eq!(nearest.kinds, [FindKind::ModuleType]);
}

#[test]
fn with_no_piece_loaded_a_control_search_says_so() {
    let mut idle = index();
    idle.set_piece_loaded(false);
    let note = "No piece is loaded, so no piece's controls were searched.";
    for kind in [None, Some(FindKind::Control)] {
        let query = FindQuery {
            kind,
            ..FindQuery::words("reverb")
        };
        assert!(idle.find(&query).unwrap().notes.contains(&note.to_string()));
    }
    let query = FindQuery {
        kind: Some(FindKind::Tool),
        ..FindQuery::words("notes")
    };
    assert!(!idle.find(&query).unwrap().notes.contains(&note.to_string()));
    assert!(!index()
        .find(&FindQuery::words("reverb"))
        .unwrap()
        .notes
        .contains(&note.to_string()));
}

#[test]
fn an_invalid_query_is_refused() {
    for query in [
        FindQuery::words("  "),
        FindQuery::words("x".repeat(MAX_FIND_WORDS_BYTES + 1)),
        FindQuery {
            limit: Some(0),
            ..FindQuery::words("reverb")
        },
        FindQuery {
            limit: Some(MAX_FIND_LIMIT + 1),
            ..FindQuery::words("reverb")
        },
    ] {
        assert!(index().find(&query).is_err(), "{query:?}");
    }
}

#[test]
fn a_query_of_stop_words_alone_still_searches() {
    let mut index = index();
    index.add(card(FindKind::Tool, "what", "What", "A tool named what."));
    assert_eq!(
        names(&index.find(&FindQuery::words("what")).unwrap())[0],
        "what"
    );
}

#[test]
fn a_query_and_its_answer_read_as_the_contract_spells_them() {
    let query: FindQuery =
        serde_json::from_value(json!({"words": "reverb", "kind": "module_type", "limit": 2}))
            .unwrap();
    assert_eq!(query.kind, Some(FindKind::ModuleType));
    let answer = serde_json::to_value(index().find(&query).unwrap()).unwrap();
    assert_eq!(answer["schema_version"], 1);
    assert_eq!(answer["cards"][0]["kind"], "module_type");
    assert!(answer.get("more").is_none());
    assert!(serde_json::from_value::<FindQuery>(json!({"words": "x", "extra": 1})).is_err());
}

#[test]
fn a_kind_left_out_is_named_when_asked_for_or_when_nothing_matched() {
    let mut index = index();
    let why = "Tools and views are searched only through the MCP find tool.";
    index.leave_out(FindKind::Tool, why);
    index.leave_out(FindKind::View, why);
    let notes = |query: FindQuery| index.find(&query).unwrap().notes;
    for kind in [FindKind::Tool, FindKind::View] {
        let query = FindQuery {
            kind: Some(kind),
            ..FindQuery::words("reverb")
        };
        assert_eq!(notes(query).iter().filter(|note| *note == why).count(), 1);
    }
    assert!(notes(FindQuery::words("zzzz")).contains(&why.to_string()));
    assert!(!notes(FindQuery::words("reverb")).contains(&why.to_string()));
    let query = FindQuery {
        kind: Some(FindKind::Example),
        ..FindQuery::words("zzzz")
    };
    assert!(!notes(query).contains(&why.to_string()));
}

#[test]
fn a_card_holding_the_word_and_what_it_means_ranks_first() {
    let mut index = Index::new();
    for key in ["bpm", "division", "feedback"] {
        index.add(
            card(
                FindKind::Control,
                &format!("echo.{key}"),
                &format!("{key} of echo (delay)"),
                "",
            )
            .with_terms(["echo", "delay"]),
        );
    }
    let answer = index.find(&FindQuery::words("more echo")).unwrap();
    assert_eq!(names(&answer)[0], "echo.feedback");
}

#[test]
fn a_card_matching_every_word_ranks_ahead_of_one_matching_some() {
    let mut index = Index::new();
    index.add(card(FindKind::Tool, "alpha_beta", "Alpha beta", ""));
    index.add(card(FindKind::Tool, "other", "Other", "alpha beta gamma"));
    let query = FindQuery {
        limit: Some(1),
        ..FindQuery::words("alpha beta gamma")
    };
    assert_eq!(index.find(&query).unwrap().cards[0].name, "other");
}

#[test]
fn a_query_spelled_like_nothing_still_gets_words_to_try() {
    let answer = index().find(&FindQuery::words("qzxqzxqzx")).unwrap();
    let nearest = answer.nearest.unwrap();
    assert!(!nearest.terms.is_empty() && nearest.terms.len() <= 3);
}

#[test]
fn notes_are_cut_short_and_counted_past_a_few() {
    let mut index = index();
    for at in 0..20 {
        index.note(format!("{at} {}", "x".repeat(MAX_FIND_BYTES)));
    }
    for kind in FindKind::ALL {
        for at in 0..200 {
            index.leave_out(kind, format!("{at} {}", "y".repeat(MAX_FIND_BYTES)));
        }
    }
    for words in ["zzzz", "filter"] {
        let answer = index.find(&FindQuery::words(words)).unwrap();
        assert!(serde_json::to_vec(&answer).unwrap().len() <= MAX_FIND_BYTES);
        assert!(answer
            .notes
            .contains(&"12 more notes were left out.".to_string()));
    }
}

#[test]
fn a_package_is_not_listed_again_after_what_it_holds() {
    let mut index = Index::new();
    let tape = "A warm tape echo.";
    index.add(card(FindKind::Development, "fugue.fx.tape", "Tape", tape));
    index.add(card(
        FindKind::Package,
        "fugue.fx.tape",
        "fugue.fx.tape",
        tape,
    ));
    let answer = index.find(&FindQuery::words("tape echo")).unwrap();
    assert_eq!(answer.cards.len(), 1);
    assert_eq!(answer.more, 0);
    let query = FindQuery {
        kind: Some(FindKind::Package),
        ..FindQuery::words("tape echo")
    };
    assert_eq!(index.find(&query).unwrap().cards[0].kind, FindKind::Package);
}
