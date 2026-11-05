use super::*;

const ABSORBED: [usize; 1] = [0];

fn cut(frames: usize) -> Option<FeedbackCut> {
    Some(FeedbackCut {
        frames,
        absorbed: &ABSORBED,
    })
}

fn route(from: usize, to: usize, port: usize) -> Edge {
    Edge {
        from,
        to,
        port: Some(port),
    }
}

fn ordering(from: usize, to: usize) -> Edge {
    Edge {
        from,
        to,
        port: None,
    }
}

#[test]
fn a_loop_through_a_delay_runs_in_chunks_of_it() {
    // 0 delays, 1 and 2 follow it in a chain back to 0's audio.
    let edges = [route(0, 1, 0), route(1, 2, 0), route(2, 0, 0)];
    let cuts = |m| (m == 0).then(|| cut(48)).flatten();
    let chunks = plan(&[0, 1, 2], &edges, cuts).expect("chunked");
    assert_eq!(chunks.frames, 48);
    assert_eq!(chunks.cuts, vec![0]);
    assert_eq!(chunks.rest, vec![1, 2]);
}

#[test]
fn the_rest_run_in_the_order_they_read_each_other() {
    // Listed out of order: 2 reads 1, which reads the delay at 0.
    let edges = [route(0, 1, 0), route(1, 2, 0), route(2, 0, 0)];
    let cuts = |m| (m == 0).then(|| cut(8)).flatten();
    let chunks = plan(&[2, 0, 1], &edges, cuts).expect("chunked");
    assert_eq!(chunks.rest, vec![1, 2]);
}

#[test]
fn the_shortest_of_several_delays_sets_the_chunk() {
    let edges = [
        route(0, 1, 0),
        route(1, 0, 0),
        route(1, 2, 0),
        route(2, 1, 1),
    ];
    let cuts = |m| match m {
        0 => cut(100),
        2 => cut(30),
        _ => None,
    };
    let chunks = plan(&[0, 1, 2], &edges, cuts).expect("chunked");
    assert_eq!(chunks.frames, 30);
    assert_eq!(chunks.cuts, vec![0, 2]);
}

#[test]
fn a_cycle_through_no_delay_runs_sample_by_sample() {
    // 1 and 2 loop between themselves without passing the delay.
    let edges = [
        route(0, 1, 0),
        route(1, 2, 0),
        route(2, 1, 1),
        route(2, 0, 0),
    ];
    let cuts = |m| (m == 0).then(|| cut(8)).flatten();
    assert!(plan(&[0, 1, 2], &edges, cuts).is_none());
}

#[test]
fn a_delay_reading_the_loop_while_it_emits_runs_sample_by_sample() {
    // 1 feeds the delay's time modulation (port 1), not its audio.
    let edges = [route(0, 1, 0), route(1, 0, 1)];
    let cuts = |m| (m == 0).then(|| cut(8)).flatten();
    assert!(plan(&[0, 1], &edges, cuts).is_none());
    // The same loop into its audio chunks.
    let edges = [route(0, 1, 0), route(1, 0, 0)];
    assert!(plan(&[0, 1], &edges, cuts).is_some());
}

#[test]
fn control_writes_inside_the_loop_run_sample_by_sample() {
    let cuts = |m| (m == 0).then(|| cut(8)).flatten();
    let into = [route(0, 1, 0), route(1, 0, 0), ordering(1, 0)];
    assert!(plan(&[0, 1], &into, cuts).is_none());
    let out = [route(0, 1, 0), route(1, 0, 0), ordering(0, 1)];
    assert!(plan(&[0, 1], &out, cuts).is_none());
    // 1 writes 2's controls, both past the delay: a chunk would apply a
    // write made late in it from its first frame.
    let between = [
        route(0, 1, 0),
        route(1, 2, 0),
        route(2, 0, 0),
        ordering(1, 2),
    ];
    assert!(plan(&[0, 1, 2], &between, cuts).is_none());
}

#[test]
fn control_writes_from_outside_the_loop_keep_its_chunks() {
    let mut groups = vec![
        ProcessGroup {
            members: vec![0],
            feedback: false,
            chunks: None,
        },
        ProcessGroup {
            members: vec![1, 2],
            feedback: true,
            chunks: None,
        },
    ];
    let edges = [
        ordering(0, 1),
        ordering(0, 2),
        route(1, 2, 0),
        route(2, 1, 0),
    ];
    let cuts = |m| (m == 1).then(|| cut(16)).flatten();
    plan_groups(true, &mut groups, &[0, 1, 1], edges, cuts);
    assert!(groups[1].chunks.is_some());
}

#[test]
fn no_declared_delay_runs_sample_by_sample() {
    let edges = [route(0, 1, 0), route(1, 0, 0)];
    assert!(plan(&[0, 1], &edges, |_| None).is_none());
    assert!(plan(&[0, 1], &edges, |m| (m == 0).then(|| cut(0)).flatten()).is_none());
}

#[test]
fn only_feedback_groups_are_planned_and_only_their_own_edges_count() {
    let mut groups = vec![
        ProcessGroup {
            members: vec![0],
            feedback: false,
            chunks: None,
        },
        ProcessGroup {
            members: vec![1, 2],
            feedback: true,
            chunks: None,
        },
    ];
    // 0 feeds the delay's time modulation from outside the loop: fine.
    let edges = [route(0, 1, 1), route(1, 2, 0), route(2, 1, 0)];
    let cuts = |m| (m == 1).then(|| cut(16)).flatten();
    plan_groups(true, &mut groups, &[0, 1, 1], edges, cuts);
    assert!(groups[0].chunks.is_none());
    let chunks = groups[1].chunks.as_ref().expect("chunked");
    assert_eq!(
        (chunks.frames, &chunks.cuts, &chunks.rest),
        (16, &vec![1], &vec![2])
    );
}

#[test]
fn the_environment_turns_chunks_off_with_0_off_or_false() {
    for off in ["0", "off", "OFF", " false "] {
        assert!(!enabled_by(Some(off)), "{off}");
    }
    for on in [None, Some("1"), Some("on"), Some("")] {
        assert!(enabled_by(on), "{on:?}");
    }
}
