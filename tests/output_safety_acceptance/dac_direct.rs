//! The DAC module driven directly: replacement, counting, finite pass-through.

use super::*;

const NAN: f32 = f32::NAN;

/// Writes `values` to the named input (the other two inputs get zeros, as a
/// graph rewrites every input each block), processes them as one block, and
/// returns the (left, right) sink block.
fn run(dac: &mut DacModule, port: &str, values: &[f32]) -> (Vec<f32>, Vec<f32>) {
    for other in ["audio", "audio_left", "audio_right"] {
        let i = dac.input_port_index(other).expect("dac has the input");
        dac.input_block_mut(i)[..values.len()].fill(0.0);
    }
    let index = dac.input_port_index(port).expect("dac has the input");
    dac.input_block_mut(index)[..values.len()].copy_from_slice(values);
    dac.process(values.len());
    let (l, r) = dac.sink_block();
    (l[..values.len()].to_vec(), r[..values.len()].to_vec())
}

/// A sink that does not count: the trait default reports zero.
struct Plain([f32; 4]);

impl Module for Plain {
    fn name(&self) -> &str {
        "Plain"
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
        &mut self.0
    }
    fn output_block(&self, _index: usize) -> &[f32] {
        &self.0
    }
    fn set_input(&mut self, port: &str, _value: f32) -> Result<(), String> {
        Err(port.to_string())
    }
    fn get_output(&self, port: &str) -> Result<f32, String> {
        Err(port.to_string())
    }
}

impl SinkModule for Plain {
    fn sink_block(&self) -> (&[f32], &[f32]) {
        (&self.0, &self.0)
    }
}

#[test]
fn non_finite_samples_defaults_to_zero_for_other_sinks() {
    let plain: &dyn SinkModule = &Plain([f32::NAN; 4]);
    assert_eq!(plain.non_finite_samples(), 0);
    let mut dac = DacModule::new();
    run(&mut dac, "audio", &[NAN]);
    let as_sink: &dyn SinkModule = &dac;
    assert_eq!(as_sink.non_finite_samples(), 2);
}
