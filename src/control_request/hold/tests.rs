use super::*;
use crate::control_request::{ControlDecl, ControlTable, DeclKind, RtValue};

const OTHER: ControlIndex = ControlIndex(4);

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "count",
        DeclKind::Integer { min: 1, max: 3 },
        RtValue::I32(3),
        "How many degrees sound",
    ),
    ControlDecl::new(
        "degree",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.0),
        "Realized from count",
    )
    .indexed(3),
    ControlDecl::new(
        "other",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.0),
        "Independent",
    ),
];

static TABLE: ControlTable = ControlTable::of(DECLS);

/// `rank`'s write due at `due`.
fn at(rank: Rank, due: u64) -> Standing {
    Standing { rank, due }
}

const USER: Rank = Rank::new(Source::User, 0);
const AGENT: Rank = Rank::new(Source::Agent, 0);
const SCRIPT: Rank = Rank::new(Source::Script, 0);
const AUTOMATED: Rank = Rank::new(Source::Automation, 0);

#[test]
fn tiers_rank_user_and_agent_over_script_over_automation_then_priority() {
    assert_eq!(USER, AGENT);
    assert!(SCRIPT < AGENT && AUTOMATED < SCRIPT);
    assert!(Rank::new(Source::Script, i16::MAX) < Rank::new(Source::Agent, i16::MIN));
    assert!(Rank::new(Source::Script, -1) < SCRIPT);
    assert_eq!(SCRIPT.loses_to(USER), Outcome::Overridden);
    assert_eq!(
        SCRIPT.loses_to(Rank::new(Source::Script, 1)),
        Outcome::Superseded
    );
}

#[test]
fn a_hold_word_keeps_its_rank_and_sample() {
    for rank in [
        USER,
        Rank::new(Source::Script, -7),
        Rank::new(Source::Agent, i16::MIN),
    ] {
        for now in [0, 1, SAMPLE_MASK] {
            assert_eq!(decode(encode(rank, now)), Some((rank, now)));
        }
    }
    assert_eq!(decode(FREE), None);
}

#[test]
fn the_higher_rank_wins_at_or_after_its_due_sample_and_past_it_the_lower_tier_takes_it_back() {
    let holds = Holds::new(TABLE.len());
    holds.take(OTHER, at(USER, 100));
    assert_eq!(
        holds.admits(OTHER, at(SCRIPT, 100)),
        Err(Outcome::Overridden)
    );
    assert_eq!(
        holds.admits(OTHER, at(AGENT, 100)),
        Ok(()),
        "equal rank: last wins"
    );
    assert_eq!(
        holds.admits(OTHER, at(SCRIPT, 101)),
        Ok(()),
        "its next event"
    );
    assert_eq!(
        holds.admits(OTHER, at(SCRIPT, 99)),
        Err(Outcome::Overridden),
        "applied late, weighed at its due sample"
    );

    holds.take(OTHER, at(Rank::new(Source::Script, 5), 200));
    assert_eq!(
        holds.admits(OTHER, at(SCRIPT, 200)),
        Err(Outcome::Superseded)
    );
    assert_eq!(
        holds.admits(OTHER, at(Rank::new(Source::Script, 6), 200)),
        Ok(())
    );
}

#[test]
fn an_index_past_the_table_holds_nothing() {
    let holds = Holds::new(TABLE.len());
    let past = ControlIndex(TABLE.len() as u16);
    holds.take(past, at(USER, 1));
    assert_eq!(holds.admits(past, at(AUTOMATED, 1)), Ok(()));
}
