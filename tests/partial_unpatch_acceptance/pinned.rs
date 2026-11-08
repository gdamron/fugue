//! A full unpatch behaves exactly as it did before partial unpatches were
//! smoothed: every sample of a document unpatched whole is pinned.
//!
//! Each case plays a held tone (or constant) whose input is patched from a
//! single source, drops that one cable at `BEFORE`, and hashes every output
//! sample's bits. The expected hashes were recorded from the full-unpatch
//! release as it stood before the partial release was added, so any change
//! to a full unpatch's samples, even the last bit, fails here.

use super::*;

fn fnv(y: &[f32]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for sample in y {
        for byte in sample.to_bits().to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

struct Case {
    name: &'static str,
    doc: Value,
    edits: Vec<(&'static str, Vec<(usize, Edit)>)>,
}

fn cases() -> Vec<Case> {
    let single = |base: (Vec<Value>, Vec<Value>), to: (&str, &str)| {
        let (mut modules, mut connections) = base;
        modules.push(constant("src"));
        connections.push(wire("src", to.0, to.1));
        document(modules, connections)
    };
    let edge = |to: (&'static str, &'static str)| Edge("src", "audio", to.0, to.1);
    vec![
        Case {
            name: "vca level on a sine",
            doc: single(
                (
                    vec![
                        tone("tone", json!({})),
                        module("amp", "vca", json!({ "level": 0.25 })),
                        dac(),
                    ],
                    vec![wire("tone", "amp", "audio"), wire("amp", "dac", "audio")],
                ),
                ("amp", "level"),
            ),
            edits: vec![
                (
                    "disconnect",
                    vec![(BEFORE, Edit::Disconnect(edge(("amp", "level"))))],
                ),
                ("remove source", vec![(BEFORE, Edit::Remove("src"))]),
            ],
        },
        Case {
            name: "vca level on a constant",
            doc: single(
                (
                    vec![
                        constant("tone"),
                        module("amp", "vca", json!({ "level": 0.25 })),
                        dac(),
                    ],
                    vec![wire("tone", "amp", "audio"), wire("amp", "dac", "audio")],
                ),
                ("amp", "level"),
            ),
            edits: vec![
                (
                    "disconnect",
                    vec![(BEFORE, Edit::Disconnect(edge(("amp", "level"))))],
                ),
                ("remove source", vec![(BEFORE, Edit::Remove("src"))]),
            ],
        },
        Case {
            name: "vca level on a constant, patched back mid-release",
            doc: single(
                (
                    vec![
                        constant("tone"),
                        module("amp", "vca", json!({ "level": 0.25 })),
                        dac(),
                    ],
                    vec![wire("tone", "amp", "audio"), wire("amp", "dac", "audio")],
                ),
                ("amp", "level"),
            ),
            edits: vec![(
                "disconnect, then connect",
                vec![
                    (BEFORE, Edit::Disconnect(edge(("amp", "level")))),
                    (2 * BLOCK, Edit::Connect(edge(("amp", "level")))),
                ],
            )],
        },
        Case {
            name: "oscillator amplitude_mod",
            doc: single(
                (
                    vec![tone("tone", json!({ "amplitude_mod_depth": 1.0 })), dac()],
                    vec![wire("tone", "dac", "audio")],
                ),
                ("tone", "amplitude_mod"),
            ),
            edits: vec![
                (
                    "disconnect",
                    vec![(BEFORE, Edit::Disconnect(edge(("tone", "amplitude_mod"))))],
                ),
                ("remove source", vec![(BEFORE, Edit::Remove("src"))]),
            ],
        },
        Case {
            name: "filter cutoff_mod",
            doc: single(
                (
                    vec![
                        tone("tone", json!({ "frequency": 1687.5 })),
                        module(
                            "flt",
                            "filter",
                            json!({ "filter_type": "lowpass", "cutoff": 500.0,
                                    "resonance": 0.0, "cutoff_mod_depth": 20000.0 }),
                        ),
                        dac(),
                    ],
                    vec![wire("tone", "flt", "audio"), wire("flt", "dac", "audio")],
                ),
                ("flt", "cutoff_mod"),
            ),
            edits: vec![
                (
                    "disconnect",
                    vec![(BEFORE, Edit::Disconnect(edge(("flt", "cutoff_mod"))))],
                ),
                ("remove source", vec![(BEFORE, Edit::Remove("src"))]),
            ],
        },
        Case {
            name: "oscillator frequency",
            doc: {
                let hz = thousand_source("src", 1.0);
                let mut modules = vec![tone("tone", json!({})), dac()];
                let mut connections = vec![wire("tone", "dac", "audio")];
                modules.extend(hz.modules);
                connections.extend(hz.wires);
                connections.push(wire("src", "tone", "frequency"));
                document(modules, connections)
            },
            edits: vec![
                (
                    "disconnect",
                    vec![(BEFORE, Edit::Disconnect(edge(("tone", "frequency"))))],
                ),
                ("remove source", vec![(BEFORE, Edit::Remove("src"))]),
            ],
        },
        Case {
            name: "audio into a vca",
            doc: document(
                vec![
                    tone("tone", json!({})),
                    module("amp", "vca", json!({ "level": 1.0 })),
                    dac(),
                ],
                vec![wire("tone", "amp", "audio"), wire("amp", "dac", "audio")],
            ),
            edits: vec![
                (
                    "disconnect",
                    vec![(
                        BEFORE,
                        Edit::Disconnect(Edge("tone", "audio", "amp", "audio")),
                    )],
                ),
                ("remove source", vec![(BEFORE, Edit::Remove("tone"))]),
            ],
        },
        Case {
            name: "audio into the dac",
            doc: document(
                vec![tone("tone", json!({})), dac()],
                vec![wire("tone", "dac", "audio")],
            ),
            edits: vec![
                (
                    "disconnect",
                    vec![(
                        BEFORE,
                        Edit::Disconnect(Edge("tone", "audio", "dac", "audio")),
                    )],
                ),
                ("remove source", vec![(BEFORE, Edit::Remove("tone"))]),
            ],
        },
    ]
}

/// (case, edit, mode, hash of every frame rendered).
const PINNED: &[(&str, &str, Mode, u64)] = &[
    (
        "vca level on a sine",
        "disconnect",
        Mode::Offline,
        0x79c3868d59785b6a,
    ),
    (
        "vca level on a sine",
        "disconnect",
        Mode::Live,
        0x20a8d4e4edf2eec2,
    ),
    (
        "vca level on a sine",
        "remove source",
        Mode::Offline,
        0x79c3868d59785b6a,
    ),
    (
        "vca level on a sine",
        "remove source",
        Mode::Live,
        0x20a8d4e4edf2eec2,
    ),
    (
        "vca level on a constant",
        "disconnect",
        Mode::Offline,
        0x2f5ea263cad8c6f7,
    ),
    (
        "vca level on a constant",
        "disconnect",
        Mode::Live,
        0x57e0f273fff7c3f7,
    ),
    (
        "vca level on a constant",
        "remove source",
        Mode::Offline,
        0x2f5ea263cad8c6f7,
    ),
    (
        "vca level on a constant",
        "remove source",
        Mode::Live,
        0x57e0f273fff7c3f7,
    ),
    (
        "vca level on a constant, patched back mid-release",
        "disconnect, then connect",
        Mode::Offline,
        0x1a3a4c21777edd6d,
    ),
    (
        "vca level on a constant, patched back mid-release",
        "disconnect, then connect",
        Mode::Live,
        0xf4b5a9ae3d82486d,
    ),
    (
        "oscillator amplitude_mod",
        "disconnect",
        Mode::Offline,
        0x4909c7b19aa56e10,
    ),
    (
        "oscillator amplitude_mod",
        "disconnect",
        Mode::Live,
        0x85ee88b6cd148e78,
    ),
    (
        "oscillator amplitude_mod",
        "remove source",
        Mode::Offline,
        0x4909c7b19aa56e10,
    ),
    (
        "oscillator amplitude_mod",
        "remove source",
        Mode::Live,
        0x85ee88b6cd148e78,
    ),
    (
        "filter cutoff_mod",
        "disconnect",
        Mode::Offline,
        0x806ad1966151443d,
    ),
    (
        "filter cutoff_mod",
        "disconnect",
        Mode::Live,
        0x806ad1966151443d,
    ),
    (
        "filter cutoff_mod",
        "remove source",
        Mode::Offline,
        0x806ad1966151443d,
    ),
    (
        "filter cutoff_mod",
        "remove source",
        Mode::Live,
        0x806ad1966151443d,
    ),
    (
        "oscillator frequency",
        "disconnect",
        Mode::Offline,
        0xe66b755a1d4ef988,
    ),
    (
        "oscillator frequency",
        "disconnect",
        Mode::Live,
        0x902ca2e91e1050e8,
    ),
    (
        "oscillator frequency",
        "remove source",
        Mode::Offline,
        0xe66b755a1d4ef988,
    ),
    (
        "oscillator frequency",
        "remove source",
        Mode::Live,
        0x902ca2e91e1050e8,
    ),
    (
        "audio into a vca",
        "disconnect",
        Mode::Offline,
        0x6fff8ccd0c583693,
    ),
    (
        "audio into a vca",
        "disconnect",
        Mode::Live,
        0x195007852941dedb,
    ),
    (
        "audio into a vca",
        "remove source",
        Mode::Offline,
        0x6fff8ccd0c583693,
    ),
    (
        "audio into a vca",
        "remove source",
        Mode::Live,
        0x195007852941dedb,
    ),
    (
        "audio into the dac",
        "disconnect",
        Mode::Offline,
        0x6fff8ccd0c583693,
    ),
    (
        "audio into the dac",
        "disconnect",
        Mode::Live,
        0x195007852941dedb,
    ),
    (
        "audio into the dac",
        "remove source",
        Mode::Offline,
        0x6fff8ccd0c583693,
    ),
    (
        "audio into the dac",
        "remove source",
        Mode::Live,
        0x195007852941dedb,
    ),
];

/// Criterion: a full unpatch behaves exactly as before, sample for sample,
/// offline and live.
#[test]
fn a_full_unpatch_is_sample_for_sample_what_it_was() {
    let mut seen = Vec::new();
    for case in cases() {
        for (how, edit) in &case.edits {
            for mode in [Mode::Offline, Mode::Live] {
                let mut rig = Rig::start(mode, &case.doc);
                let mut y = Vec::new();
                for (frames, edit) in edit {
                    y.extend(rig.render(*frames));
                    rig.apply(edit);
                }
                y.extend(rig.render(AFTER));
                assert!(y.iter().all(|v| v.is_finite()));
                seen.push((case.name, *how, mode, fnv(&y)));
            }
        }
    }
    let mut wrong = Vec::new();
    for &(name, how, mode, hash) in &seen {
        match PINNED
            .iter()
            .find(|p| p.0 == name && p.1 == how && p.2 == mode)
        {
            Some(p) if p.3 == hash => {}
            Some(p) => wrong.push(format!(
                "{name} / {mode:?} / {how}: hash {hash:#018x}, pinned {:#018x}",
                p.3
            )),
            None => wrong.push(format!(
                "{name} / {mode:?} / {how}: hash {hash:#018x}, unpinned"
            )),
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}
