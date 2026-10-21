//! Each instance an edit removes or replaces is told it left the running
//! graph (see [`Module::leave`]), as the edit installs.

use std::sync::atomic::AtomicUsize;

use super::*;

/// A port-less module counting the times it is told it left.
struct Leaving(Arc<AtomicUsize>);

impl Module for Leaving {
    fn name(&self) -> &str {
        "leaving"
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
        &mut []
    }

    fn output_block(&self, _index: usize) -> &[f32] {
        &[]
    }

    fn set_input(&mut self, _port: &str, _value: f32) -> Result<(), String> {
        Err(String::new())
    }

    fn get_output(&self, _port: &str) -> Result<f32, String> {
        Err(String::new())
    }

    fn leave(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn leaving(count: &Arc<AtomicUsize>) -> ModuleInstance {
    GraphModule::Module(Box::new(Leaving(count.clone())))
}

#[test]
fn an_edit_tells_each_instance_it_removes_or_replaces_that_it_left() {
    let [removed, replaced, replacement] = [(); 3].map(|()| Arc::new(AtomicUsize::new(0)));
    let mut modules = IndexMap::new();
    modules.insert("removed".to_string(), leaving(&removed));
    modules.insert("replaced".to_string(), leaving(&replaced));
    modules.insert("dac".to_string(), build("dac", serde_json::json!({})));
    let sinks = sinks_of(&modules);
    let mut graph = SignalGraph::new(modules, sinks, Vec::new(), MasterObservers::default());
    graph.recompile();
    let ends = link(&mut graph, 4);
    render(&mut graph, 1);
    let mut next = publication(
        vec![
            ("replaced", Next::Prepared(leaving(&replacement))),
            ("dac", Next::Survivor("dac")),
        ],
        Vec::new(),
    );
    next.map_survivors(["removed", "replaced", "dac"]);
    ends.publish(next);

    let ((), allocs, frees) = allocator_events(|| graph.ensure_process_order());
    assert_eq!((allocs, frees), (0, 0), "installing touched the allocator");
    assert_eq!(ids(&graph), ["replaced", "dac"]);
    let told = [&removed, &replaced, &replacement].map(|count| count.load(Ordering::Relaxed));
    assert_eq!(told, [1, 1, 0], "removed, replaced, its replacement");
}
