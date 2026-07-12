//! FUG-188 acceptance check: with the sustain pedal held, notes ring past
//! their gate ends on their own decay — with the reverb fully dry, so the
//! tail demonstrably comes from the voices, not a room.
//!
//! The pedal model is built from small, explicitly wired modules: a
//! `sustain` module inside each voice (gate + pedal → gate) and a `divisi`
//! allocator fanning the note line across an explicit bank of voices. A
//! sequencer plays two notes (C4 then G4) into the bank while a second
//! sequencer lane holds the pedal for the whole render — the "simple case"
//! from the issue: a gate lane driven into the bank's `pedal` input, which
//! fans out to every voice by explicit connections. Long after both note
//! gates end, both pitches must still be sounding above a floor; without the
//! pedal lane the same patch is silent there.

use fugue::RenderEngine;

const SAMPLE_RATE: u32 = 44_100;

/// Two-note invention. Each voice's envelope has sustain level 0 (a struck
/// voice decays to silence even while held) and a release (20 ms) far
/// shorter than its decay (2.5 s), so only the pedal can make a note outlive
/// its gate. `pedaled` patches the pedal lane's gate into the bank's pedal
/// input.
fn invention(pedaled: bool) -> String {
    let pedal_connection = if pedaled {
        r#",{ "from": "seq_pedal", "from_port": "gate", "to": "bank", "to_port": "pedal" }"#
    } else {
        ""
    };

    // The bank: divisi fans the line across four explicit ring_voice
    // instances; the pedal input fans out to each voice's sustain module.
    let mut bank_modules = vec![
        r#"{ "id": "div", "type": "divisi", "config": { "voices": 4 } }"#.to_string(),
    ];
    let mut bank_connections = Vec::new();
    let mut bank_inputs = vec![
        r#"{ "name": "frequency", "to": "div", "to_port": "frequency" }"#.to_string(),
        r#"{ "name": "gate", "to": "div", "to_port": "gate" }"#.to_string(),
    ];
    for n in 1..=4 {
        bank_modules.push(format!(r#"{{ "id": "v{n}", "type": "ring_voice" }}"#));
        bank_connections.push(format!(
            r#"{{ "from": "div", "from_port": "frequency{n}", "to": "v{n}", "to_port": "frequency" }}"#
        ));
        bank_connections.push(format!(
            r#"{{ "from": "div", "from_port": "gate{n}", "to": "v{n}", "to_port": "gate" }}"#
        ));
        bank_connections.push(format!(
            r#"{{ "from": "v{n}", "from_port": "audio", "to": "mix", "to_port": "in{n}" }}"#
        ));
        bank_inputs.push(format!(
            r#"{{ "name": "pedal", "to": "v{n}", "to_port": "pedal" }}"#
        ));
    }
    bank_modules.push(
        r#"{ "id": "mix", "type": "mixer", "config": { "channels": 4, "levels": [1.0, 1.0, 1.0, 1.0] } }"#
            .to_string(),
    );

    format!(
        r#"{{
        "version": "1.0.0",
        "developments": [
            {{
                "name": "ring_bank",
                "definition": {{
                    "developments": [
                        {{ "name": "ring_voice", "definition": {{
                            "modules": [
                                {{ "id": "osc", "type": "oscillator", "config": {{ "oscillator_type": "sine" }} }},
                                {{ "id": "sus", "type": "sustain" }},
                                {{ "id": "env", "type": "adsr",
                                   "config": {{ "attack": 0.002, "decay": 2.5, "sustain": 0.0, "release": 0.02 }} }},
                                {{ "id": "vca", "type": "vca" }}
                            ],
                            "connections": [
                                {{ "from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio" }},
                                {{ "from": "sus", "from_port": "gate", "to": "env", "to_port": "gate" }},
                                {{ "from": "env", "from_port": "envelope", "to": "vca", "to_port": "cv" }}
                            ],
                            "inputs": [
                                {{ "name": "frequency", "to": "osc", "to_port": "frequency" }},
                                {{ "name": "gate", "to": "sus", "to_port": "gate" }},
                                {{ "name": "pedal", "to": "sus", "to_port": "pedal" }}
                            ],
                            "outputs": [
                                {{ "name": "audio", "from": "vca", "from_port": "audio" }}
                            ]
                        }} }}
                    ],
                    "modules": [ {bank_modules} ],
                    "connections": [ {bank_connections} ],
                    "inputs": [ {bank_inputs} ],
                    "outputs": [
                        {{ "name": "audio", "from": "mix", "from_port": "left" }}
                    ]
                }}
            }}
        ],
        "modules": [
            {{ "id": "clock", "type": "clock", "config": {{ "bpm": 240.0, "gate_duration": 0.5 }} }},
            {{ "id": "seq_notes", "type": "cell_sequencer",
               "config": {{ "base_note": 60, "steps": 16, "gate_length": 0.5, "mode": "one_shot",
                            "sequences": [[null, 0, 7, null, null, null, null, null,
                                           null, null, null, null, null, null, null, null]] }} }},
            {{ "id": "seq_pedal", "type": "cell_sequencer",
               "config": {{ "base_note": 60, "steps": 16, "gate_length": 1.0, "mode": "one_shot",
                            "sequences": [[ {{ "note": 0 }}, {{ "held": true }}, {{ "held": true }}, {{ "held": true }},
                                            {{ "held": true }}, {{ "held": true }}, {{ "held": true }}, {{ "held": true }},
                                            {{ "held": true }}, {{ "held": true }}, {{ "held": true }}, {{ "held": true }},
                                            {{ "held": true }}, {{ "held": true }}, {{ "held": true }}, {{ "held": true }} ]] }} }},
            {{ "id": "bank", "type": "ring_bank", "config": {{}} }},
            {{ "id": "reverb", "type": "reverb",
               "config": {{ "room_size": 0.85, "decay": 0.8, "damping": 0.35, "wet": 0.0, "dry": 1.0 }} }},
            {{ "id": "dac", "type": "dac", "config": {{ "soft_clip": false }} }}
        ],
        "connections": [
            {{ "from": "clock", "from_port": "gate", "to": "seq_notes", "to_port": "gate" }},
            {{ "from": "clock", "from_port": "gate", "to": "seq_pedal", "to_port": "gate" }},
            {{ "from": "seq_notes", "from_port": "frequency", "to": "bank", "to_port": "frequency" }},
            {{ "from": "seq_notes", "from_port": "gate", "to": "bank", "to_port": "gate" }},
            {{ "from": "bank", "from_port": "audio", "to": "reverb", "to_port": "left" }},
            {{ "from": "bank", "from_port": "audio", "to": "reverb", "to_port": "right" }},
            {{ "from": "reverb", "from_port": "left", "to": "dac", "to_port": "audio_left" }},
            {{ "from": "reverb", "from_port": "right", "to": "dac", "to_port": "audio_right" }}
            {pedal_connection}
        ]
    }}"#,
        bank_modules = bank_modules.join(",\n"),
        bank_connections = bank_connections.join(",\n"),
        bank_inputs = bank_inputs.join(",\n"),
        pedal_connection = pedal_connection
    )
}

/// Renders `seconds` of the invention and returns the left channel.
fn render(json: &str, seconds: f32) -> Vec<f32> {
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_json(json).unwrap();
    let frames = (seconds * SAMPLE_RATE as f32) as usize;
    let mut interleaved = vec![0.0f32; frames * 2];
    engine.render_interleaved(&mut interleaved).unwrap();
    interleaved.iter().step_by(2).copied().collect()
}

/// Goertzel power of `signal` at `frequency`, normalized by window length —
/// a per-pitch "is this note sounding" probe.
fn goertzel(signal: &[f32], frequency: f32) -> f32 {
    let omega = 2.0 * std::f64::consts::PI * f64::from(frequency) / f64::from(SAMPLE_RATE);
    let coefficient = 2.0 * omega.cos();
    let (mut previous, mut before_previous) = (0.0f64, 0.0f64);
    for &sample in signal {
        let current = f64::from(sample) + coefficient * previous - before_previous;
        before_previous = previous;
        previous = current;
    }
    let power = previous * previous + before_previous * before_previous
        - coefficient * previous * before_previous;
    (power / (signal.len() as f64 * signal.len() as f64)) as f32
}

fn rms(signal: &[f32]) -> f32 {
    let sum: f64 = signal.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    ((sum / signal.len() as f64) as f32).sqrt()
}

/// The pattern opens with a rest so the sequencer has measured the true step
/// duration before the first note. Note gates: C4 spans 0.25–0.375 s, G4
/// spans 0.5–0.625 s (240 BPM quarters, gate_length 0.5). The probe window
/// sits well past both.
const WINDOW_START: f32 = 1.0;
const WINDOW_END: f32 = 1.4;

fn window(signal: &[f32]) -> &[f32] {
    let start = (WINDOW_START * SAMPLE_RATE as f32) as usize;
    let end = (WINDOW_END * SAMPLE_RATE as f32) as usize;
    &signal[start..end]
}

#[test]
fn pedal_held_notes_ring_past_gate_end_with_reverb_dry() {
    let left = render(&invention(true), 1.7);
    let tail = window(&left);

    // Both notes ended their gates ~0.6 s in; at 1.0–1.4 s they still ring.
    let floor = 1e-4;
    let c4 = goertzel(tail, 261.626);
    let g4 = goertzel(tail, 391.995);
    assert!(
        c4 > floor,
        "C4 should still be sounding past its gate end, power = {c4:e}"
    );
    assert!(
        g4 > floor,
        "G4 should still be sounding past its gate end, power = {g4:e}"
    );

    // A pitch away from both notes stays far below them: the tail is the two
    // ringing notes, not broadband reverb wash.
    let off = goertzel(tail, 554.365); // C#5
    assert!(
        off < c4.min(g4) * 0.05,
        "off-pitch energy should be far below the ringing notes: off = {off:e}, c4 = {c4:e}, g4 = {g4:e}"
    );
}

#[test]
fn without_pedal_the_same_patch_is_silent_there() {
    let left = render(&invention(false), 1.7);
    let tail = window(&left);
    let level = rms(tail);
    assert!(
        level < 1e-5,
        "with a 20 ms release and a dry reverb the tail window must be silent, rms = {level}"
    );
}
