/// Computes strongly-connected components of `adj` (downstream adjacency) via
/// an iterative Tarjan's algorithm.
///
/// Returns `(comp_id, comps)` where `comp_id[node]` is the node's component
/// index and `comps` lists each component's members in **topological order**
/// (sources before sinks).
pub(super) fn tarjan_scc(adj: &[Vec<usize>], n: usize) -> (Vec<usize>, Vec<Vec<usize>>) {
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
