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
fn a_hold_keeps_automation_off_until_its_next_event() {
    let holds = Holds::new(TABLE.len());
    assert!(!holds.holds_off_automation(OTHER));
    holds.take(OTHER, at(SCRIPT, 1));
    assert!(holds.holds_off_automation(OTHER));
    holds.release(OTHER, 0);
    assert!(!holds.holds_off_automation(OTHER));

    // An automation-tier request is automation's next event too.
    holds.take(OTHER, at(USER, 2));
    holds.take(OTHER, at(AUTOMATED, 3));
    assert!(!holds.holds_off_automation(OTHER));
}

#[test]
fn an_entry_at_or_before_a_holds_sample_loses_to_it() {
    let holds = Holds::new(TABLE.len());
    holds.take(OTHER, at(USER, 64));
    {
        let _zero = FrameScope::enter(60);
        holds.release(OTHER, 4);
        assert!(holds.holds_off_automation(OTHER), "at the write's sample");
        let _per_sample = FrameScope::frame(4);
        holds.release(OTHER, 0);
        assert!(holds.holds_off_automation(OTHER), "a sample at a time");
        holds.release(OTHER, 1);
        assert!(!holds.holds_off_automation(OTHER), "a sample after it");
    }
    holds.take(OTHER, at(USER, 64));
    holds.release(OTHER, 0);
    assert!(!holds.holds_off_automation(OTHER), "outside processing");
}

#[test]
fn an_index_past_the_table_holds_nothing() {
    let holds = Holds::new(TABLE.len());
    let past = ControlIndex(TABLE.len() as u16);
    holds.take(past, at(USER, 1));
    assert!(!holds.holds_off_automation(past));
    assert_eq!(holds.admits(past, at(AUTOMATED, 1)), Ok(()));
}
