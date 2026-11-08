//! Signal sources that put a known value on a cable, and the assembly of a
//! document whose one input is fed by two of them.

use super::*;

/// A signal source: the modules it needs, their wiring, and the module
/// whose `audio` output is the cable.
pub struct Source {
    pub modules: Vec<Value>,
    pub wires: Vec<Value>,
    pub out: &'static str,
}

/// A constant `level`: a still square through a vca whose level is `level`.
/// The vca named `id` is the module whose output is the cable.
pub fn constant_source(id: &'static str, level: f64) -> Source {
    let k: &'static str = Box::leak(format!("k_{id}").into_boxed_str());
    Source {
        modules: vec![constant(k), module(id, "vca", json!({ "level": level }))],
        wires: vec![wire(k, id, "audio")],
        out: id,
    }
}

/// A sine of exactly 256 samples a cycle at amplitude `level`, in phase
/// with every other such source.
pub fn tone_source(id: &'static str, level: f64) -> Source {
    let t: &'static str = Box::leak(format!("t_{id}").into_boxed_str());
    Source {
        modules: vec![
            tone(t, json!({})),
            module(id, "vca", json!({ "level": level })),
        ],
        wires: vec![wire(t, id, "audio")],
        out: id,
    }
}

/// A constant of `level` x 1000: three +20 dB gains after the constant.
/// The last gain, `id`, is the module whose output is the cable.
pub fn thousand_source(id: &'static str, level: f64) -> Source {
    let k: &'static str = Box::leak(format!("k_{id}").into_boxed_str());
    let v: &'static str = Box::leak(format!("v_{id}").into_boxed_str());
    let g1: &'static str = Box::leak(format!("g1_{id}").into_boxed_str());
    let g2: &'static str = Box::leak(format!("g2_{id}").into_boxed_str());
    let gain = |name: &str| module(name, "gain", json!({ "decibels": 20.0 }));
    Source {
        modules: vec![
            constant(k),
            module(v, "vca", json!({ "level": level })),
            gain(g1),
            gain(g2),
            gain(id),
        ],
        wires: vec![
            wire(k, v, "audio"),
            wire(v, g1, "audio"),
            wire(g1, g2, "audio"),
            wire(g2, id, "audio"),
        ],
        out: id,
    }
}

/// How much of the fan-in a document has.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cables {
    /// Both sources, both cables.
    Both,
    /// Both sources, only the first cable.
    WithoutCable,
    /// Only the first source and cable.
    WithoutSource,
}

/// `base` plus sources `a` and `b` both wired into (`to`, `port`), as far
/// as `cables` says.
pub fn fan_in(
    base: (Vec<Value>, Vec<Value>),
    a: &Source,
    b: &Source,
    (to, port): (&str, &str),
    cables: Cables,
) -> Value {
    let (mut modules, mut connections) = base;
    modules.extend(a.modules.clone());
    connections.extend(a.wires.clone());
    connections.push(wire(a.out, to, port));
    if cables != Cables::WithoutSource {
        modules.extend(b.modules.clone());
        connections.extend(b.wires.clone());
    }
    if cables == Cables::Both {
        connections.push(wire(b.out, to, port));
    }
    document(modules, connections)
}
