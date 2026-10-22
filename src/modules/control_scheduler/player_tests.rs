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
    module.set_step(2);
    let counted = module.counted();
    assert_eq!(
        (counted.step, counted.since, counted.measured),
        (2, 0, false)
    );
}
