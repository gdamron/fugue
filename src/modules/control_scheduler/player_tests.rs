//! Who plays the schedule: told the drain plays it on its edges, the
//! scheduler counts them (its `step` output, and the count a drain takes
//! up) and writes nothing; told it plays again, it plays on from the step
//! counted.

use super::tests::{pulse, setup};
use super::*;

const SCHEDULE: &str = r#"[
    { "at_step": 1, "module": "mixer", "control": "level.0", "value": 0.5 },
    { "at_step": 3, "module": "mixer", "control": "level.0", "value": 0.25 }
]"#;

#[test]
fn played_on_its_edges_it_counts_them_and_writes_nothing() {
    let (mut module, mixer, _dir) = setup(SCHEDULE);
    assert_eq!(module.counted().step, -1, "no edge yet");
    module.set_played(Player::Edges);
    pulse(&mut module, 9); // step 0
    pulse(&mut module, 4); // step 1
    assert_eq!(mixer.level(0), 1.0, "the drain writes its entries");
    assert_eq!(module.get_output("step").unwrap(), 1.0);
    let counted = Counted {
        step: 1,
        gate: 0.0,
        reset: 0.0,
        since: 5,
        interval: 10,
        measured: true,
    };
    assert_eq!(module.counted(), counted);
}

#[test]
fn played_again_by_itself_it_plays_on_from_the_step_counted() {
    let (mut module, mixer, _dir) = setup(SCHEDULE);
    module.set_played(Player::Edges);
    pulse(&mut module, 3); // step 0
    pulse(&mut module, 3); // step 1, played by the drain
    module.set_played(Player::Module);
    pulse(&mut module, 3); // step 2
    assert_eq!(mixer.level(0), 1.0, "step 1 had its edge");
    pulse(&mut module, 3); // step 3
    assert_eq!(mixer.level(0), 0.25);
}

#[test]
fn moved_off_a_clock_it_reports_the_step_the_drain_told() {
    let (mut module, _mixer, _dir) = setup(SCHEDULE);
    module.set_played(Player::Clock);
    module.process(4);
    module.set_step(2, false);
    let counted = module.counted();
    assert_eq!(
        (counted.step, counted.since, counted.measured),
        (2, 0, false)
    );
}

/// Moved off a clock on the sample of a step the drain told, the clock's
/// gate rising there is that step, not another: playing on by itself,
/// or on the drain's edges, the scheduler counts the next rise as the
/// next step.
#[test]
fn moved_off_a_clock_on_a_step_s_sample_counts_that_gate_once() {
    for player in [Player::Module, Player::Edges] {
        let (mut module, _mixer, _dir) = setup(SCHEDULE);
        module.set_played(Player::Clock);
        module.set_step(0, true);
        module.process(4);
        // The clock's next gate rises on the next block's first sample:
        // the drain tells step 1 there, and the scheduler moves off it.
        module.set_step(1, true);
        module.set_input("clock", 1.0).unwrap();
        module.set_played(player);
        if player == Player::Edges {
            assert_eq!(module.counted().gate, 1.0, "the drain takes it as counted");
        }
        module.process(1);
        assert_eq!(module.get_output("step").unwrap(), 1.0, "{player:?}");
        module.set_input("clock", 0.0).unwrap();
        module.process(3);
        pulse(&mut module, 3);
        assert_eq!(module.get_output("step").unwrap(), 2.0, "{player:?}");
    }
}

/// A clock reset on its beat tells a lower step, or the same one, on that
/// gate's sample: moved off it there, the scheduler still counts that gate
/// once, on its own or on the drain's edges.
#[test]
fn moved_off_a_clock_reset_on_its_beat_counts_that_gate_once() {
    for (from, player) in [
        (5, Player::Module),
        (0, Player::Module),
        (5, Player::Edges),
        (0, Player::Edges),
    ] {
        let (mut module, _mixer, _dir) = setup(SCHEDULE);
        module.set_played(Player::Clock);
        module.set_step(from, true);
        module.process(4);
        module.set_step(0, true);
        module.set_input("clock", 1.0).unwrap();
        module.set_played(player);
        if player == Player::Edges {
            assert_eq!(module.counted().gate, 1.0, "the drain takes it as counted");
        }
        module.process(1);
        assert_eq!(module.get_output("step").unwrap(), 0.0, "{from} {player:?}");
        module.set_input("clock", 0.0).unwrap();
        module.process(3);
        pulse(&mut module, 3);
        assert_eq!(module.get_output("step").unwrap(), 1.0, "{from} {player:?}");
    }
}

/// Moved off a clock on the next step's sample, before the drain told it
/// (it tells none on the segment it moves a schedule off its clock), the
/// scheduler counts that rise as the step.
#[test]
fn moved_off_a_clock_before_a_step_was_told_counts_its_rise() {
    for player in [Player::Module, Player::Edges] {
        let (mut module, _mixer, _dir) = setup(SCHEDULE);
        module.set_played(Player::Clock);
        module.set_step(0, true);
        module.process(4);
        module.set_input("clock", 1.0).unwrap();
        module.set_played(player);
        if player == Player::Edges {
            assert_eq!(module.counted().gate, 0.0, "the drain counts the rise");
        }
        module.process(1);
        assert_eq!(module.get_output("step").unwrap(), 1.0, "{player:?}");
    }
}

/// Moved off a clock mid-step, the scheduler counts the next rise there
/// as the next step.
#[test]
fn moved_off_a_clock_mid_step_counts_the_next_rise() {
    let (mut module, _mixer, _dir) = setup(SCHEDULE);
    module.set_played(Player::Clock);
    module.set_step(0, true);
    module.process(4);
    module.set_step(0, false);
    module.set_input("clock", 1.0).unwrap();
    module.set_played(Player::Module);
    module.process(1);
    assert_eq!(module.get_output("step").unwrap(), 1.0);
}

#[test]
fn an_unconnected_input_holds_what_was_written_to_it() {
    let (mut module, _mixer, _dir) = setup(SCHEDULE);
    assert_eq!(module.held_input(1), 0.0);
    module.set_input("reset", 1.0).unwrap();
    assert_eq!((module.held_input(0), module.held_input(1)), (0.0, 1.0));
}
