//! The output mix glides to each write, as a mixer's fader does, and the
//! mix a reverb is built with holds from its first frame.

use super::*;
use crate::alloc_counter::allocator_events;
use crate::audio_thread::AudioThreadScope;

const RATE: u32 = 44_100;

/// A reverb at `wet` 0, so its output is the dry input times `dry`, fed a
/// constant 1.0.
fn dry_only() -> Reverb {
    let mut reverb = Reverb::new(RATE);
    reverb.set_control("wet", 0.0).unwrap();
    reverb.wet.snap(0.0);
    reverb
}

#[test]
fn a_dry_write_glides_without_a_step_or_an_allocation() {
    let mut reverb = dry_only();
    let glide = gain_ramp_frames(RATE) as usize;
    reverb.set_control("dry", 0.0).unwrap();
    let mut heard = Vec::with_capacity(glide);
    let ((), allocs, frees) = allocator_events(|| {
        let _audio = AudioThreadScope::enter();
        for _ in 0..glide {
            reverb.set_input("audio_left", 1.0).unwrap();
            reverb.process(1);
            heard.push(reverb.get_output("audio_left").unwrap());
        }
    });
    assert_eq!((allocs, frees), (0, 0));
    let step = 1.0 / glide as f32;
    let mut previous = 1.0;
    for (frame, &value) in heard.iter().enumerate() {
        assert!(
            (previous - value - step).abs() < step * 0.01,
            "frame {frame}: {previous} -> {value}"
        );
        previous = value;
    }
    assert_eq!(heard[glide - 1], 0.0, "it lands on the level");
}

#[test]
fn wet_and_width_writes_glide_too() {
    for key in ["wet", "width"] {
        let mut reverb = Reverb::new(RATE);
        reverb.set_control(key, 0.0).unwrap();
        let ramp = if key == "wet" {
            &reverb.wet
        } else {
            &reverb.width
        };
        assert!(ramp.is_gliding(), "{key} glides");
        for _ in 0..gain_ramp_frames(RATE) {
            reverb.process(1);
        }
        let ramp = if key == "wet" {
            &reverb.wet
        } else {
            &reverb.width
        };
        assert_eq!(ramp.value(), 0.0, "{key} lands on the write");
    }
}

#[test]
fn a_configured_mix_holds_from_the_first_frame() {
    let result = ReverbFactory
        .build(
            RATE,
            &serde_json::json!({ "wet": 0.0, "dry": 0.5, "width": 0.25 }),
        )
        .unwrap();
    let GraphModule::Module(mut reverb) = result.module else {
        panic!("reverb is a module");
    };
    reverb.set_input("audio_left", 1.0).unwrap();
    reverb.process(1);
    assert_eq!(reverb.get_output("audio_left").unwrap(), 0.5);
}
