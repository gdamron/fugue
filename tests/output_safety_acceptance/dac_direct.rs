//! The DAC module driven directly: replacement, counting, finite pass-through.

use super::*;

const NAN: f32 = f32::NAN;
const INF: f32 = f32::INFINITY;

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

/// What the DAC writes for one finite sample on `audio_left`.
fn finite_reference(x: f32) -> f32 {
    run(&mut DacModule::new(), "audio_left", &[x]).0[0]
}

#[test]
fn nonfinite_on_one_channel_is_silence_and_the_other_is_untouched() {
    let input = [NAN, INF, -INF, 0.25, -0.5, 3.0];
    for (port, other) in [("audio_left", 1usize), ("audio_right", 0)] {
        let mut dac = DacModule::new();
        let (l, r) = run(&mut dac, port, &input);
        let (hit, quiet) = if other == 1 { (l, r) } else { (r, l) };
        for (i, &x) in input.iter().enumerate() {
            if x.is_finite() {
                assert_eq!(
                    hit[i].to_bits(),
                    finite_reference(x).to_bits(),
                    "{port}[{i}]"
                );
            } else {
                assert_eq!(hit[i], 0.0, "{port}[{i}] ({x}) is silence");
            }
            assert_eq!(quiet[i], 0.0, "the other channel stays silent");
        }
        assert_eq!(
            dac.non_finite_samples(),
            3,
            "{port}: three samples replaced"
        );
    }
    assert_eq!(finite_reference(0.25), 0.25);
    assert!(
        (0.99..1.0).contains(&finite_reference(3.0)),
        "still soft-clipped"
    );
}

#[test]
fn nonfinite_mono_input_silences_both_channels_and_counts_twice() {
    let mut dac = DacModule::new();
    let (l, r) = run(&mut dac, "audio", &[NAN, 0.25, INF, -0.5]);
    for (i, want) in [(0, None), (1, Some(0.25f32)), (2, None), (3, Some(-0.5))] {
        match want {
            None => assert_eq!((l[i], r[i]), (0.0, 0.0), "frame {i} silent on both"),
            Some(v) => assert_eq!((l[i], r[i]), (v, v), "frame {i} summed into both"),
        }
    }
    assert_eq!(dac.non_finite_samples(), 4, "two frames, two channels each");
}

#[test]
fn replacement_holds_with_soft_clip_off_and_finite_values_pass_exactly() {
    let mut dac = DacModule::new().with_soft_clip(false);
    let (l, _) = run(&mut dac, "audio_left", &[NAN, 3.0, -INF, -2.0]);
    assert_eq!(l, [0.0, 3.0, 0.0, -2.0]);
    assert_eq!(dac.non_finite_samples(), 2);
}

#[test]
fn count_accumulates_across_blocks_and_per_module_and_starts_at_zero() {
    let mut a = DacModule::new();
    let b = DacModule::default();
    assert_eq!(a.non_finite_samples(), 0);
    run(&mut a, "audio_left", &[0.1, 0.2, 0.3]);
    assert_eq!(a.non_finite_samples(), 0, "finite samples are not counted");
    run(&mut a, "audio_left", &[NAN, 0.2]);
    run(&mut a, "audio_right", &[0.1, INF]);
    assert_eq!(a.non_finite_samples(), 2, "running total over blocks");
    run(&mut a, "audio", &[NAN]);
    assert_eq!(a.non_finite_samples(), 4);
    assert_eq!(b.non_finite_samples(), 0, "counts belong to their module");
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
