use super::ProcessGroup;

/// Computes strongly-connected components of `adj` (downstream adjacency) via
/// an iterative Tarjan's algorithm.
///
/// Returns `(comp_id, comps)` where `comp_id[node]` is the node's component
/// index and `comps` lists each component's members in **topological order**
/// (sources before sinks).
pub(crate) fn tarjan_scc(adj: &[Vec<usize>], n: usize) -> (Vec<usize>, Vec<Vec<usize>>) {
    const UNVISITED: usize = usize::MAX;

    let mut index = vec![UNVISITED; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut comp_id = vec![UNVISITED; n];
    let mut scc_stack: Vec<usize> = Vec::new();
    let mut comps: Vec<Vec<usize>> = Vec::new();
    let mut next_index = 0usize;

    for start in 0..n {
        if index[start] != UNVISITED {
            continue;
        }

        // Explicit call stack of (node, next-child-cursor).
        let mut call_stack: Vec<(usize, usize)> = vec![(start, 0)];
        while let Some((v, cursor)) = call_stack.last_mut() {
            let v = *v;
            if *cursor == 0 {
                index[v] = next_index;
                low[v] = next_index;
                next_index += 1;
                scc_stack.push(v);
                on_stack[v] = true;
            }

            if *cursor < adj[v].len() {
                let w = adj[v][*cursor];
                *cursor += 1;
                if index[w] == UNVISITED {
                    call_stack.push((w, 0));
                } else if on_stack[w] && index[w] < low[v] {
                    low[v] = index[w];
                }
            } else {
                // Finished exploring v's children.
                if low[v] == index[v] {
                    let mut comp = Vec::new();
                    loop {
                        let w = scc_stack.pop().unwrap();
                        on_stack[w] = false;
                        comp_id[w] = comps.len();
                        comp.push(w);
                        if w == v {
                            break;
                        }
                    }
                    comps.push(comp);
                }
                call_stack.pop();
                if let Some((parent, _)) = call_stack.last() {
                    let parent = *parent;
                    if low[v] < low[parent] {
                        low[parent] = low[v];
                    }
                }
            }
        }
    }

    // Tarjan finalizes SCCs in reverse topological order; flip to topological.
    comps.reverse();
    let total = comps.len();
    for id in comp_id.iter_mut() {
        if *id != UNVISITED {
            *id = total - 1 - *id;
        }
    }

    (comp_id, comps)
}

/// Reverse post-order of a depth-first walk along `adj` (downstream
/// adjacency), starting from each unvisited node in index order and
/// following edges in list order: a topological order when `adj` has no
/// loop, and the per-sample order of a feedback group's members when it
/// has.
pub(crate) fn topological_order(adj: &[Vec<usize>]) -> Vec<usize> {
    topological_order_from(adj, &[])
}

/// [`topological_order`], with the walk starting from `first`, in order,
/// before the rest in index order: where the graph is entered decides
/// which route of a loop it reaches last, and so which is delayed.
pub(crate) fn topological_order_from(adj: &[Vec<usize>], first: &[usize]) -> Vec<usize> {
    let n = adj.len();
    // 0 = unvisited, 1 = on stack (in progress), 2 = finished.
    let mut state = vec![0_u8; n];
    let mut order: Vec<usize> = Vec::with_capacity(n);

    for start in first.iter().copied().chain(0..n) {
        if state[start] != 0 {
            continue;
        }

        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        state[start] = 1;

        while let Some((node, idx)) = stack.last_mut() {
            let node = *node;
            if *idx < adj[node].len() {
                let next = adj[node][*idx];
                *idx += 1;
                // Back-edge (on stack) or already finished: skip.
                if state[next] == 0 {
                    state[next] = 1;
                    stack.push((next, 0));
                }
            } else {
                let (finished, _) = stack.pop().unwrap();
                state[finished] = 2;
                order.push(finished);
            }
        }
    }

    order.reverse();
    order
}

/// Each node's position in `order`.
pub(crate) fn positions(order: &[usize]) -> Vec<usize> {
    let mut pos = vec![0usize; order.len()];
    for (i, &m) in order.iter().enumerate() {
        pos[m] = i;
    }
    pos
}

/// The process groups of `adj`: its strongly-connected components in
/// topological order, each member list ordered by `pos` (see
/// [`topological_order`]), and a group a feedback group when it has more
/// than one member or a self-loop. Returns the groups and the group of
/// each node.
pub(crate) fn process_groups(adj: &[Vec<usize>], pos: &[usize]) -> (Vec<ProcessGroup>, Vec<usize>) {
    let n = adj.len();
    let (_, comps) = tarjan_scc(adj, n);
    let groups: Vec<ProcessGroup> = comps
        .into_iter()
        .map(|mut members| {
            members.sort_by_key(|&m| pos[m]);
            let feedback = members.len() > 1 || adj[members[0]].contains(&members[0]);
            ProcessGroup {
                members,
                feedback,
                chunks: None,
            }
        })
        .collect();
    let mut group_of = vec![0; n];
    for (g, group) in groups.iter().enumerate() {
        for &m in &group.members {
            group_of[m] = g;
        }
    }
    (groups, group_of)
}

/// Whether a route from `from` to `to` is a back-edge: both in one group,
/// with the source processed at or after the destination in the
/// per-sample order, so the destination reads the source's previous
/// sample.
pub(crate) fn is_back_edge(group_of: &[usize], pos: &[usize], from: usize, to: usize) -> bool {
    group_of[from] == group_of[to] && pos[from] >= pos[to]
}
