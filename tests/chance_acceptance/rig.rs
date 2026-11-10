//! Shared rig: builds `chance` through the public registry and drives its
//! clock by hand.
//!
//! Every step is one block of four samples, `[high, high, low, low]` on the
//! `clock` input, so each block holds exactly one rising edge, on its first
//! sample. Outputs are read on the block's second sample, where a fired gate
//! (jitter 0) is still high.

use fugue::{ControlValue, GraphModule, ModuleRegistry};

pub const SAMPLE_RATE: u32 = 48_000;
pub const BLOCK: usize = 4;
pub const READ_AT: usize = 1;

pub const OUTPUTS: [&str; 9] = [
    "gate_1",
    "gate_2",
    "tick",
    "frequency_1",
    "frequency_2",
    "frequency_3",
    "value_1",
    "value_2",
    "value_3",
];

/// One step's worth of outputs, compared by bit pattern.
#[derive(Clone, Copy, Debug)]
pub struct Step {
    pub gate: [f32; 2],
    pub tick: f32,
    pub freq: [f32; 3],
    pub value: [f32; 3],
}

impl Step {
    pub fn bits(&self) -> [u32; 9] {
        [
            self.gate[0].to_bits(),
            self.gate[1].to_bits(),
            self.tick.to_bits(),
            self.freq[0].to_bits(),
            self.freq[1].to_bits(),
            self.freq[2].to_bits(),
            self.value[0].to_bits(),
            self.value[1].to_bits(),
            self.value[2].to_bits(),
        ]
    }

    /// The outputs that take a new value on every step: both gates, the tick
    /// and channel 3.
    pub fn every_step_bits(&self) -> [u32; 5] {
        [
            self.gate[0].to_bits(),
            self.gate[1].to_bits(),
            self.tick.to_bits(),
            self.freq[2].to_bits(),
            self.value[2].to_bits(),
        ]
    }

    pub fn fired(&self, gate: usize) -> bool {
        self.gate[gate] > 0.5
    }
}

pub struct Rig {
    module: GraphModule,
    clock: usize,
    reset: usize,
    out: [usize; 9],
}

pub fn rig(config: serde_json::Value) -> Rig {
    let built = ModuleRegistry::default()
        .build("chance", SAMPLE_RATE, &config)
        .unwrap_or_else(|e| panic!("chance should build from {config}: {e}"));
    let mut module = built.module;
    let m = module.module_mut();
    let clock = m.input_port_index("clock").expect("input port `clock`");
    let reset = m.input_port_index("reset").expect("input port `reset`");
    let mut out = [0usize; 9];
    for (slot, name) in out.iter_mut().zip(OUTPUTS) {
        *slot = m
            .output_port_index(name)
            .unwrap_or_else(|| panic!("output port `{name}`"));
    }
    m.set_input_connected(clock, true);
    Rig {
        module,
        clock,
        reset,
        out,
    }
}

impl Rig {
    pub fn block(&mut self, clock: [f32; BLOCK], reset: [f32; BLOCK]) {
        let (c, r) = (self.clock, self.reset);
        let m = self.module.module_mut();
        m.input_block_mut(c)[..BLOCK].copy_from_slice(&clock);
        m.input_block_mut(r)[..BLOCK].copy_from_slice(&reset);
        m.process(BLOCK);
    }

    pub fn read(&self, port: usize, at: usize) -> f32 {
        self.module.module().output_block(self.out[port])[at]
    }

    pub fn snapshot(&self, at: usize) -> Step {
        Step {
            gate: [self.read(0, at), self.read(1, at)],
            tick: self.read(2, at),
            freq: [self.read(3, at), self.read(4, at), self.read(5, at)],
            value: [self.read(6, at), self.read(7, at), self.read(8, at)],
        }
    }

    /// One clock pulse; returns the outputs on the block's second sample.
    pub fn step(&mut self) -> Step {
        self.block([1.0, 1.0, 0.0, 0.0], [0.0; BLOCK]);
        self.snapshot(READ_AT)
    }

    pub fn run(&mut self, steps: usize) -> Vec<Step> {
        (0..steps).map(|_| self.step()).collect()
    }

    /// Writes a control through the module itself with `Module::set_control`.
    /// A module built outside a graph has no bound control surface (writes to
    /// it are staged until it joins one), so the write goes to the module.
    /// Numbers pass through; a choice is written as its option position in
    /// the interface's listed order (`KEYS`, `SCALES`).
    pub fn set(&mut self, key: &str, value: ControlValue) {
        let number = match (key, &value) {
            (_, ControlValue::Number(n)) => *n,
            ("key", ControlValue::String(name)) => {
                position(KEYS.iter().map(|(k, _)| *k), name) as f32
            }
            ("scale", ControlValue::String(name)) => {
                position(SCALES.iter().map(|(s, _)| *s), name) as f32
            }
            _ => panic!("no numeric form for {key} = {value:?}"),
        };
        self.module
            .module_mut()
            .set_control(key, number)
            .unwrap_or_else(|e| panic!("setting {key}: {e}"));
    }
}

fn position<'a>(mut options: impl Iterator<Item = &'a str>, name: &str) -> usize {
    options
        .position(|option| option == name)
        .unwrap_or_else(|| panic!("`{name}` is not a listed option"))
}

/// Keys in the interface's listed order, with their pitch class.
pub const KEYS: [(&str, i32); 12] = [
    ("C", 0),
    ("C#", 1),
    ("D", 2),
    ("D#", 3),
    ("E", 4),
    ("F", 5),
    ("F#", 6),
    ("G", 7),
    ("G#", 8),
    ("A", 9),
    ("A#", 10),
    ("B", 11),
];

/// Scales in the interface's listed order, with their semitone intervals.
pub const SCALES: [(&str, &[i32]); 14] = [
    ("major", &[0, 2, 4, 5, 7, 9, 11]),
    ("minor", &[0, 2, 3, 5, 7, 8, 10]),
    ("harmonic_minor", &[0, 2, 3, 5, 7, 8, 11]),
    ("melodic_minor", &[0, 2, 3, 5, 7, 9, 11]),
    ("dorian", &[0, 2, 3, 5, 7, 9, 10]),
    ("phrygian", &[0, 1, 3, 5, 7, 8, 10]),
    ("lydian", &[0, 2, 4, 6, 7, 9, 11]),
    ("mixolydian", &[0, 2, 4, 5, 7, 9, 10]),
    ("locrian", &[0, 1, 3, 5, 6, 8, 10]),
    ("major_pentatonic", &[0, 2, 4, 7, 9]),
    ("minor_pentatonic", &[0, 3, 5, 7, 10]),
    ("blues", &[0, 3, 5, 6, 7, 10]),
    ("whole_tone", &[0, 2, 4, 6, 8, 10]),
    ("chromatic", &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
];
