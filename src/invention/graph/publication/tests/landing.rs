//! Timed edits on the audio side (see `publication::landing`), queued by
//! hand as the publisher queues them.

use super::*;
use crate::control_request::{ControlTarget, RequestValue, RtValue, When};

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

/// The `frequency` input block of module `id`.
fn frequency(graph: &mut SignalGraph, id: &str) -> Vec<f32> {
    let module = graph.modules.get_mut(id).unwrap().module_mut();
    let port = module.input_port_index("frequency").unwrap();
    module.input_block_mut(port).to_vec()
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
    // The retired publication stays as the bridge, not yet retired.
    assert!(ends.retired.pop().is_none());
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

#[test]
fn the_bridge_maps_what_was_submitted_against_the_replaced_graph() {
    let mut graph = base_graph();
    graph.request_hook = Some(SignalGraph::request_as_input_write);
    let mut ends = link(&mut graph, 4);
    let port = frequency_port(&graph);
    time(&ends, rebuild_osc1(), When::AtSample(10));
    render(&mut graph, 1);
    assert_eq!(ids(&graph), ["osc2", "dac", "osc1"]);

    // Tagged with the replaced generation (osc1 at 0, osc2 at 1): what
    // reaches a survivor is mapped, what reaches the rebuilt osc1 dropped.
    let write = |module_idx, value| InputWrite {
        generation: 0,
        module_idx,
        port_idx: port,
        value,
    };
    ends.inputs.push(write(1, 0.25)).unwrap();
    ends.inputs.push(write(0, 0.5)).unwrap();
    assert_eq!(counted_block(&mut graph), (0, 0));
    assert!(frequency(&mut graph, "osc2").iter().all(|v| *v == 0.25));
    assert!(frequency(&mut graph, "osc1").iter().all(|v| *v == 0.0));
    let request = |module_idx, value| {
        let target = ControlTarget {
            generation: 0,
            module_idx,
            control: crate::control_request::ControlIndex(port as u16),
        };
        Request::new(target, RequestValue::Value(RtValue::F32(value)))
    };
    let requests = &ends.requests;
    requests.submit(request(1, 0.75)).unwrap();
    requests.submit(request(0, 0.5)).unwrap();
    assert_eq!(counted_block(&mut graph), (0, 0));
    assert!(frequency(&mut graph, "osc2").iter().all(|v| *v == 0.75));
    assert!(frequency(&mut graph, "osc1").iter().all(|v| *v == 0.0));

    // Kept until the publisher adopts the edit; then handed back.
    render(&mut graph, 2);
    assert!(ends.retired.pop().is_none());
    let published = graph.requests.as_ref().unwrap().published().clone();
    published.store(1, Ordering::Release);
    assert_eq!(counted_block(&mut graph), (0, 0));
    let bridge = ends.retired.pop().expect("the bridge went back");
    assert_eq!(bridge.generation, 1);
    assert!(graph.link.as_ref().unwrap().bridge.is_none());
}

#[test]
fn an_edit_after_a_landing_installs_once_the_bridge_goes() {
    let mut graph = base_graph();
    let ends = link(&mut graph, 4);
    time(&ends, rebuild_osc1(), When::AtSample(10));
    render(&mut graph, 1);

    // The publisher adopted the landing, then published on top of it.
    let mut next = publication(
        vec![
            ("osc2", Next::Survivor("oscillator")),
            ("dac", Next::Survivor("dac")),
        ],
        vec![edge("osc2", "dac", "audio")],
    );
    next.map_survivors(["osc2", "dac", "osc1"]);
    next.generation = 2;
    let published = graph.requests.as_ref().unwrap().published().clone();
    published.store(2, Ordering::Release);
    ends.requests
        .submit(Request::edit(1, Payload::owned(next)))
        .unwrap();

    assert_eq!(counted_block(&mut graph), (0, 0));
    assert_eq!(ids(&graph), ["osc2", "dac"]);
    assert_eq!(ends.applied.load(Ordering::Relaxed), 2);
    let retired: Vec<_> = std::iter::from_fn(|| ends.retired.pop()).collect();
    let generations: Vec<_> = retired.iter().map(|p| p.generation).collect();
    assert_eq!(generations, [1, 2]);
}

/// A write of `value` to osc `module_idx`'s frequency input, tagged
/// `generation`, due `when`.
fn frequency_request(
    graph: &SignalGraph,
    generation: u64,
    module_idx: usize,
    value: f32,
) -> Request {
    let target = ControlTarget {
        generation,
        module_idx,
        control: crate::control_request::ControlIndex(frequency_port(graph) as u16),
    };
    Request::new(target, RequestValue::Value(RtValue::F32(value)))
}

/// osc2 and the dac, osc1 gone: the publisher's next change on top of
/// [`rebuild_osc1`], adopted.
fn drop_osc1(ends: &ControlEnds, graph: &SignalGraph) {
    let mut next = publication(
        vec![
            ("osc2", Next::Survivor("oscillator")),
            ("dac", Next::Survivor("dac")),
        ],
        vec![edge("osc2", "dac", "audio")],
    );
    next.map_survivors(["osc2", "dac", "osc1"]);
    next.generation = 2;
    graph
        .requests
        .as_ref()
        .unwrap()
        .published()
        .store(2, Ordering::Release);
    ends.requests
        .submit(Request::edit(1, Payload::owned(next)))
        .unwrap();
}

#[test]
fn writes_for_a_newer_generation_never_keep_the_bridge() {
    let mut graph = base_graph();
    let mut ends = link(&mut graph, 4);
    let port = frequency_port(&graph);
    time(&ends, rebuild_osc1(), When::AtSample(10));
    render(&mut graph, 1);
    drop_osc1(&ends, &graph);
    // Enough writes for the edit behind the bridge to fill the held ones.
    for _ in 0..16 {
        let write = InputWrite {
            generation: 2,
            module_idx: 0,
            port_idx: port,
            value: 0.5,
        };
        if ends.inputs.push(write).is_err() {
            break;
        }
    }

    assert_eq!(counted_block(&mut graph), (0, 0));
    assert_eq!(ids(&graph), ["osc2", "dac"]);
    assert!(frequency(&mut graph, "osc2").iter().all(|v| *v == 0.5));
}

#[test]
fn an_edit_behind_a_bridge_is_not_starved_by_a_full_store() {
    let mut graph = base_graph();
    graph.request_hook = Some(SignalGraph::request_as_input_write);
    let ends = link(&mut graph, 4);
    time(&ends, rebuild_osc1(), When::AtSample(10));
    render(&mut graph, 1);
    // Far-off writes, submitted against the replaced graph, fill the store.
    let far = |n: u64| When::AtSample((1 << 40) + n);
    for n in 0..16 {
        let mut request = frequency_request(&graph, 0, 1, 0.25);
        request.when = far(n);
        ends.requests.submit(request).unwrap();
    }
    render(&mut graph, 1);
    // One more waits at the head, then the publisher adopts and edits.
    let mut blocked = frequency_request(&graph, 0, 1, 0.25);
    blocked.when = far(16);
    ends.requests.submit(blocked).unwrap();
    render(&mut graph, 1);
    assert_eq!(ends.applied.load(Ordering::Relaxed), 1);
    drop_osc1(&ends, &graph);

    assert_eq!(counted_block(&mut graph), (0, 0));
    assert_eq!(ids(&graph), ["osc2", "dac"]);
    assert_eq!(ends.applied.load(Ordering::Relaxed), 2);
}

#[test]
fn a_bridged_write_replaces_one_in_a_full_store_without_room() {
    let mut graph = base_graph();
    graph.request_hook = Some(SignalGraph::request_as_input_write);
    let ends = link(&mut graph, 4);
    time(&ends, rebuild_osc1(), When::AtSample(10));
    render(&mut graph, 1);
    // Writes for osc2 in block 6, against the replaced graph, fill the
    // store, mapped as the landing installed.
    let at = |n: u64| When::AtSample(FRAMES as u64 * 6 + n);
    for n in 0..16 {
        let mut request = frequency_request(&graph, 0, 1, 0.25);
        request.when = at(n);
        ends.requests.submit(request).unwrap();
    }
    render(&mut graph, 1);
    // Still tagged with the replaced generation, it replaces the last.
    let mut again = frequency_request(&graph, 0, 1, 0.5);
    again.when = at(15);
    ends.requests.submit(again).unwrap();
    // Then adopted, with nothing else to install: no edit waits.
    let drain = graph.requests.as_ref().unwrap();
    drain.published().store(1, Ordering::Release);

    // Through block 6, the input held at the value written last.
    render(&mut graph, 5);
    assert!(frequency(&mut graph, "osc2").iter().all(|v| *v == 0.5));
}

#[test]
fn a_bridged_write_already_expired_replaces_nothing() {
    let mut graph = base_graph();
    graph.request_hook = Some(SignalGraph::request_as_input_write);
    let ends = link(&mut graph, 4);
    time(&ends, rebuild_osc1(), When::AtSample(10));
    render(&mut graph, 1);
    // Both against the replaced graph, for osc2 at one sample; the second
    // expires before it.
    let at = When::AtSample(FRAMES as u64 * 2);
    let mut kept = frequency_request(&graph, 0, 1, 0.25);
    kept.when = at;
    let mut late = frequency_request(&graph, 0, 1, 0.5);
    (late.when, late.ttl) = (at, Some(1));
    ends.requests.submit(kept).unwrap();
    ends.requests.submit(late).unwrap();

    render(&mut graph, 2);
    assert!(frequency(&mut graph, "osc2").iter().all(|v| *v == 0.25));
}
