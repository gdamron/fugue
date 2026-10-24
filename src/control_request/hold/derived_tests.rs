//! Holds on controls a write rewrites within its own module: a source
//! write is weighed against them and holds them, and a control that only
//! brings others into play holds and overwrites none.

use super::*;
use crate::control_request::{ControlDecl, ControlTable, DeclKind, Refusal, RtValue};
use crate::MAX_BLOCK;

/// Rewrites every degree (and names the pulse, which it must not hold).
const SHIFT: ControlIndex = ControlIndex(0);
const DEGREE: [ControlIndex; 3] = [ControlIndex(1), ControlIndex(2), ControlIndex(3)];
/// Decides how many degrees are in play; rewrites none.
const COUNT: ControlIndex = ControlIndex(4);
const PULSE: ControlIndex = ControlIndex(5);

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "shift",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.0),
        "Rewrites every degree",
    ),
    ControlDecl::new(
        "degree",
        DeclKind::Number { min: 0.0, max: 1.0 },
        RtValue::F32(0.0),
        "One position",
    )
    .indexed(3),
    ControlDecl::new(
        "count",
        DeclKind::Integer { min: 1, max: 3 },
        RtValue::I32(3),
        "How many degrees sound",
    ),
    ControlDecl::new("pulse", DeclKind::Bool, RtValue::Bool(false), "An event").event(),
];

static TABLE: ControlTable = ControlTable::of(DECLS);

const USER: Standing = Standing::user(10);
const SCRIPT: Standing = Standing {
    rank: Rank::new(Source::Script, 0),
    due: 10,
};

struct Scale {
    cells: ControlCells,
    out: [f32; MAX_BLOCK],
}

impl Scale {
    fn new() -> Self {
        Self {
            cells: ControlCells::new(
                DECLS
                    .iter()
                    .flat_map(|decl| std::iter::repeat_n(decl.default, usize::from(decl.count))),
            ),
            out: [0.0; MAX_BLOCK],
        }
    }

    fn holds(&self) -> &Holds {
        &self.cells.holds
    }
}

impl Module for Scale {
    fn name(&self) -> &str {
        "Scale"
    }

    fn process(&mut self, _frames: usize) -> bool {
        true
    }

    fn inputs(&self) -> &[&str] {
        &[]
    }

    fn outputs(&self) -> &[&str] {
        &[]
    }

    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        &mut self.out
    }

    fn output_block(&self, _index: usize) -> &[f32] {
        &self.out
    }

    fn set_input(&mut self, port: &str, _value: f32) -> Result<(), String> {
        Err(format!("no input '{port}'"))
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        Err(format!("no output '{port}'"))
    }

    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    fn apply(&mut self, _control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        Ok(value)
    }

    fn for_each_alias(
        &self,
        control: ControlIndex,
        f: &mut dyn FnMut(&ControlTable, &ControlCells, ControlIndex),
    ) {
        if control == SHIFT {
            for index in DEGREE.into_iter().chain([PULSE]) {
                f(&TABLE, &self.cells, index);
            }
        }
    }
}

#[test]
fn a_rewriting_write_holds_every_control_it_rewrites_but_never_an_event() {
    let scale = Scale::new();
    take_hold(&scale, SHIFT, USER);
    for index in [SHIFT].into_iter().chain(DEGREE) {
        assert!(scale.holds().holds_off_automation(index), "{index:?}");
    }
    assert!(!scale.holds().holds_off_automation(PULSE));
    assert!(!scale.holds().holds_off_automation(COUNT));
    assert!(
        kept_hold(&scale, SHIFT, USER),
        "an unheld event keeps nothing from it"
    );

    // A position written since is no longer the rewrite's.
    take_hold(&scale, DEGREE[1], Standing::user(11));
    assert!(!kept_hold(&scale, SHIFT, USER));
}

#[test]
fn a_rewriting_write_loses_to_a_higher_hold_on_any_control_it_rewrites() {
    let scale = Scale::new();
    take_hold(&scale, DEGREE[2], USER);
    assert_eq!(admits_hold(&scale, SHIFT, SCRIPT), Err(Outcome::Overridden));
    assert_eq!(admits_hold(&scale, SHIFT, USER), Ok(()), "equal rank");
    assert_eq!(
        admits_hold(&scale, COUNT, SCRIPT),
        Ok(()),
        "the count rewrites no position"
    );
}

/// The strongest loss decides, wherever the positions sit.
#[test]
fn a_rewriting_writes_outcome_is_its_strongest_loss_in_any_order() {
    let script_high = Standing {
        rank: Rank::new(Source::Script, 5),
        ..SCRIPT
    };
    for (superseding, overriding) in [(DEGREE[0], DEGREE[2]), (DEGREE[2], DEGREE[0])] {
        let scale = Scale::new();
        take_hold(&scale, superseding, script_high);
        take_hold(&scale, overriding, USER);
        assert_eq!(admits_hold(&scale, SHIFT, SCRIPT), Err(Outcome::Overridden));
    }
}

/// Moving a control that only brings others into play holds none of them,
/// so automation keeps writing a position, and a position held above it
/// stays held: the hold wins.
#[test]
fn a_count_write_neither_holds_nor_frees_a_position() {
    let scale = Scale::new();
    take_hold(&scale, DEGREE[0], USER);
    take_hold(&scale, COUNT, Standing::user(20));
    assert!(scale.holds().holds_off_automation(DEGREE[0]));
    assert!(!scale.holds().holds_off_automation(DEGREE[1]));
    assert!(kept_hold(&scale, DEGREE[0], USER));
}
