//! Timed edits on the audio side (see `publication::landing`), queued by
//! hand as the publisher queues them.

use super::*;
use crate::control_request::When;

/// Queues `publication` as a timed edit due `when`, prepared against the
/// generation before it, as the publisher queues one.
fn time(ends: &ControlEnds, mut publication: Box<Publication>, when: When) -> Fate {
    let fate = Fate::default();
    publication.fate = Some(fate.guard());
    let mut edit = Request::edit(publication.generation - 1, Payload::owned(publication));
    edit.target.module_idx = TIMED_EDIT_MODULE;
    edit.when = when;
    ends.requests.submit(edit).unwrap();
    fate
}

/// Only the dac, so the output is silent from the sample it installs on.
fn silence() -> Box<Publication> {
    let mut next = publication(vec![("dac", Next::Survivor("dac"))], vec![]);
    next.map_survivors(BASE_IDS);
    next.generation = 1;
    next
}

/// osc2 and the dac survive, moved to the front; osc1 is rebuilt.
fn rebuild_osc1() -> Box<Publication> {
    let mut next = publication(
        vec![
            ("osc2", Next::Survivor("oscillator")),
            ("dac", Next::Survivor("dac")),
            ("osc1", Next::Prepared(osc(220.0))),
        ],
        vec![edge("osc1", "dac", "audio"), edge("osc2", "dac", "audio")],
    );
    next.map_survivors(BASE_IDS);
    next.generation = 1;
    next
}

#[test]
fn a_timed_edit_lands_on_its_sample_mid_block_without_touching_the_allocator() {
    let mut graph = base_graph();
    let ends = link(&mut graph, 4);
    render(&mut graph, 1);
    let due = FRAMES as u64 + 10;
    let fate = time(&ends, silence(), When::AtSample(due));

    let mut left = [0.0f32; FRAMES];
    let mut right = [0.0f32; FRAMES];
    let ((), allocs, frees) = allocator_events(|| graph.process_block(&mut left, &mut right));
    assert_eq!((allocs, frees), (0, 0), "landing touched the allocator");
    assert!(fate.landed());
    assert_eq!(ids(&graph), ["dac"]);
    assert_eq!(ends.applied.load(Ordering::Relaxed), 1);
    // Playing up to the sample it is due on, silent from it on.
    assert_ne!(left[9], 0.0);
    assert!(left[10..].iter().all(|v| *v == 0.0), "{:?}", &left[8..14]);
    // It goes back, holding the graph it replaced, to be freed.
    assert!(ends.retired.pop().is_some());
}

#[test]
fn a_cancelled_timed_edit_never_lands() {
    let mut graph = base_graph();
    let ends = link(&mut graph, 4);
    let fate = time(&ends, silence(), When::AtSample(10));
    assert!(fate.cancel());

    let out = render(&mut graph, 2);
    assert!(!fate.landed());
    assert_eq!(ids(&graph), BASE_IDS);
    assert_eq!(ends.applied.load(Ordering::Relaxed), 0);
    assert!(out[10..].iter().any(|v| *v != 0.0));
}

#[test]
fn an_edit_installing_first_refuses_a_timed_one() {
    let mut graph = base_graph();
    let ends = link(&mut graph, 4);
    let fate = time(&ends, silence(), When::AtSample(FRAMES as u64 * 2));
    render(&mut graph, 1);
    // Prepared against the same graph, published later, installed first.
    let mut first = rebuild_osc1();
    first.generation = 1;
    ends.requests
        .submit(Request::edit(0, Payload::owned(first)))
        .unwrap();

    render(&mut graph, 3);
    assert!(!fate.landed());
    assert_eq!(ids(&graph), ["osc2", "dac", "osc1"]);
    assert_eq!(ends.applied.load(Ordering::Relaxed), 1);
}
