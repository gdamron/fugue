//! The pattern as a payload: clock edges read it without a lock, a clone
//! of the pattern, an allocation or a free, and a written pattern is taken
//! whole, the one it replaces handed back to be retired.

use super::*;
use crate::alloc_counter::allocator_events;
use crate::payload::{Payload, Retired};

fn notes(offsets: &[i8]) -> Vec<Step> {
    offsets.iter().map(|&offset| Step::note(offset)).collect()
}

/// Feeds `edges` clock periods of `period` samples, one sample at a time,
/// returning the frequency at each edge.
fn clocked(seq: &mut StepSequencer, edges: usize, period: usize) -> Vec<f32> {
    let mut heard = Vec::with_capacity(edges);
    for edge in 0..edges * period {
        seq.set_input("clock", if edge % period < period / 2 { 1.0 } else { 0.0 })
            .unwrap();
        seq.process(1);
        if edge % period == 0 {
            heard.push(seq.get_output("frequency").unwrap());
        }
    }
    heard
}

fn hz(offset: i16) -> f32 {
    Note::new((i16::from(DEFAULT_ROOT_NOTE) + offset) as u8).frequency()
}

/// Many steps, with grace notes, held steps and a write every few edges,
/// on one thread counting what it allocates and frees. The payloads are
/// built, and what they replace freed, outside the count.
#[test]
fn clock_edges_and_pattern_writes_neither_allocate_nor_free() {
    let mut first = notes(&[0, 2, 4, 5, 7, 9, 11, 12]);
    first[3] = Step::note_with_grace(5, &[7, 9]);
    first[6] = Step::held();
    let mut seq = StepSequencer::new(1000)
        .with_step_count(8)
        .with_pattern(first);
    let mut payloads: Vec<Payload> = (0..16)
        .map(|n| Payload::new(notes(&[n, n + 1, n + 2][..1 + n as usize % 3])))
        .collect();
    let mut retired: Vec<Retired> = Vec::with_capacity(payloads.len());
    let ((), allocs, frees) = allocator_events(|| {
        for block in 0..400 {
            for i in 0..64 {
                let clock = if (block * 64 + i) % 50 < 25 { 1.0 } else { 0.0 };
                seq.inputs.block_mut(0)[i] = clock;
            }
            seq.process(64);
            if block % 25 == 24 {
                let payload = payloads.pop().unwrap();
                retired.push(seq.apply_payload(controls::PATTERN, payload).unwrap());
            }
        }
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(retired.len(), 16);
}

#[test]
fn a_written_pattern_plays_from_the_next_edge() {
    let mut seq = StepSequencer::new(1000)
        .with_step_count(4)
        .with_pattern(notes(&[0, 2, 4, 5]));
    assert_eq!(clocked(&mut seq, 2, 10), [hz(0), hz(2)]);
    let pattern = Payload::new(notes(&[10, 11, 12, 13]));
    let replaced = seq.apply_payload(controls::PATTERN, pattern).unwrap();
    assert_eq!(
        seq.get_output("frequency").unwrap(),
        hz(2),
        "step 1 sounds on"
    );
    assert_eq!(clocked(&mut seq, 3, 10), [hz(12), hz(13), hz(10)]);
    drop(replaced);
}

/// A shorter pattern does not shorten `step_count`: steps past its end
/// rest, the step index it was at included, and the count still loops.
#[test]
fn a_pattern_shorter_than_the_step_playing_rests_past_its_end() {
    let mut seq = StepSequencer::new(1000)
        .with_step_count(6)
        .with_pattern(notes(&[0, 1, 2, 3, 4, 5]));
    clocked(&mut seq, 5, 10);
    assert_eq!(seq.current_step(), 4);
    let replaced = seq.apply_payload(controls::PATTERN, Payload::new(notes(&[7, 9])));
    drop(replaced.unwrap());
    assert_eq!(seq.step_count(), 6);
    assert_eq!(seq.get_control("step_count").unwrap(), 6.0);
    let heard = clocked(&mut seq, 4, 10);
    assert_eq!(
        heard,
        [0.0, hz(7), hz(9), 0.0],
        "step 5 rests, then 0, 1, 2"
    );
    assert_eq!(seq.current_step(), 2);
}

#[test]
fn a_payload_of_another_kind_is_refused_and_handed_back() {
    let mut seq = StepSequencer::new(1000).with_pattern(notes(&[3]));
    let Err((refusal, payload)) = seq.apply_payload(controls::PATTERN, Payload::new(5u8)) else {
        panic!("refused");
    };
    assert_eq!(refusal, Refusal::Invalid);
    assert!(payload.downcast::<u8>().is_ok());
    let pattern = Payload::new(notes(&[4]));
    let Err((refusal, _)) = seq.apply_payload(controls::MODE, pattern) else {
        panic!("refused");
    };
    assert_eq!(refusal, Refusal::Unsupported);
    assert_eq!(clocked(&mut seq, 1, 10), [hz(3)]);
}

/// The pattern reads back as JSON text, listed after `gate_length` as it
/// always was, and is not one of the module's own number controls.
#[test]
fn the_pattern_reads_back_where_and_as_it_always_has() {
    let built = StepSequencerFactory
        .build(
            1000,
            &serde_json::json!({ "pattern": [{ "note": 0, "gate_length": 0.8 }, null] }),
        )
        .unwrap();
    let surface = built.control_surface.unwrap();
    let keys: Vec<_> = surface
        .controls()
        .into_iter()
        .map(|meta| meta.key)
        .collect();
    assert_eq!(
        keys,
        [
            "root_note",
            "step_count",
            "gate_length",
            "pattern",
            "mode",
            "grace_duration",
            "grace_placement",
            "ended"
        ]
    );
    let shown = r#"[{"note":0,"gate_length":0.8},{"note":null}]"#;
    assert_eq!(surface.get_control("pattern").unwrap(), shown.into());
    let listed = ControlMeta::string("pattern", "Step pattern as JSON")
        .with_default(crate::ControlValue::from(shown));
    assert_eq!(surface.controls()[3], listed);
    let module = built.module.module();
    assert!(module.controls().iter().all(|meta| meta.key != "pattern"));
    assert!(module.get_control("pattern").is_err());
}

/// The hand-numbered indices name the controls the table declares.
#[test]
fn control_indices_follow_the_table() {
    let named = [
        ("root_note", controls::ROOT_NOTE),
        ("step_count", controls::STEP_COUNT),
        ("gate_length", controls::GATE_LENGTH),
        ("pattern", controls::PATTERN),
        ("mode", controls::MODE),
        ("grace_duration", controls::GRACE_DURATION),
        ("grace_placement", controls::GRACE_PLACEMENT),
        ("ended", controls::ENDED),
    ];
    for (key, index) in named {
        assert_eq!(TABLE.resolve(key), Some(index), "{key}");
    }
    assert_eq!(TABLE.len(), named.len());
}

/// One development key cannot fan a pattern out to two sequencers: each
/// would retire a pattern for one request.
#[test]
fn a_development_key_reaching_two_patterns_is_refused() {
    let document = serde_json::json!({ "version": "1.0.0",
        "developments": [{ "name": "pair", "definition": { "version": "1.0.0",
            "modules": [{ "id": "a", "type": "step_sequencer" }, { "id": "b", "type": "step_sequencer" }],
            "connections": [],
            "controls": [{ "name": "notes", "module": "a", "control": "pattern" },
                         { "name": "notes", "module": "b", "control": "pattern" }] } }],
        "modules": [{ "id": "p", "type": "pair" }], "connections": [] });
    let document = crate::Invention::from_json(&document.to_string()).unwrap();
    let built = crate::InventionBuilder::new(1000).build(document);
    let error = built.err().unwrap().to_string();
    assert!(error.contains("exactly one"), "{error}");
}
