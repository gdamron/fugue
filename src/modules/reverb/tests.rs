use super::*;

mod mix_glide;

#[test]
fn test_silence_in_silence_out() {
    let mut reverb = Reverb::new(44100);
    for _ in 0..2000 {
        reverb.process(1);
        let l = reverb.get_output("audio_left").unwrap();
        let r = reverb.get_output("audio_right").unwrap();
        assert!(!l.is_nan(), "Left output is NaN");
        assert!(!r.is_nan(), "Right output is NaN");
        assert!(l.is_finite(), "Left output is infinite");
        assert!(r.is_finite(), "Right output is infinite");
    }
}

#[test]
fn test_impulse_produces_reverb_tail() {
    let mut reverb = Reverb::new(44100);
    reverb.set_control("wet", 1.0).unwrap();
    reverb.set_control("dry", 0.0).unwrap();

    // Feed a single impulse
    reverb.set_input("audio_left", 1.0).unwrap();
    reverb.process(1);
    reverb.set_input("audio_left", 0.0).unwrap();

    // Check for non-zero output in the tail
    let mut found_output = false;
    for _ in 0..8000 {
        reverb.process(1);
        let l = reverb.get_output("audio_left").unwrap();
        let r = reverb.get_output("audio_right").unwrap();
        if l.abs() > 1e-6 || r.abs() > 1e-6 {
            found_output = true;
            break;
        }
    }
    assert!(found_output, "Expected reverb tail after impulse");
}

#[test]
fn test_dry_passthrough() {
    let mut reverb = Reverb::new(44100);
    reverb.set_control("wet", 0.0).unwrap();
    reverb.set_control("dry", 1.0).unwrap();
    // Silence while the mix glides to the writes.
    for _ in 0..gain_ramp_frames(44100) {
        reverb.process(1);
    }

    reverb.set_input("audio_left", 0.75).unwrap();
    reverb.set_input("audio_right", -0.5).unwrap();
    reverb.process(1);

    let l = reverb.get_output("audio_left").unwrap();
    let r = reverb.get_output("audio_right").unwrap();
    assert!(
        (l - 0.75).abs() < 1e-6,
        "Dry passthrough left: expected 0.75, got {}",
        l
    );
    assert!(
        (r - (-0.5)).abs() < 1e-6,
        "Dry passthrough right: expected -0.5, got {}",
        r
    );
}

#[test]
fn test_freeze_sustains_output() {
    let mut reverb = Reverb::new(44100);
    reverb.set_control("wet", 1.0).unwrap();
    reverb.set_control("dry", 0.0).unwrap();

    // Feed signal
    for _ in 0..2000 {
        reverb.set_input("audio_left", 0.5).unwrap();
        reverb.process(1);
    }
    reverb.set_input("audio_left", 0.0).unwrap();

    // Enable freeze
    reverb.set_control("freeze", 1.0).unwrap();

    // Output should remain non-zero (frozen feedback)
    let mut energy = 0.0f32;
    for _ in 0..4000 {
        reverb.process(1);
        energy += reverb.get_output("audio_left").unwrap().abs();
    }
    assert!(
        energy > 1.0,
        "Freeze mode should sustain output, got total energy {}",
        energy
    );
}

#[test]
fn test_no_denormal_explosion() {
    let mut reverb = Reverb::new(44100);
    reverb.set_control("wet", 1.0).unwrap();
    reverb.set_control("decay", 0.9).unwrap();

    // Feed a brief signal then silence
    for _ in 0..100 {
        reverb.set_input("audio_left", 0.1).unwrap();
        reverb.process(1);
    }
    reverb.set_input("audio_left", 0.0).unwrap();

    // Run many samples of silence — output should stay bounded
    for i in 0..50000 {
        reverb.process(1);
        let l = reverb.get_output("audio_left").unwrap();
        let r = reverb.get_output("audio_right").unwrap();
        assert!(
            l.abs() < 100.0,
            "Left output exploded at sample {}: {}",
            i,
            l
        );
        assert!(
            r.abs() < 100.0,
            "Right output exploded at sample {}: {}",
            i,
            r
        );
    }
}

#[test]
fn test_controls() {
    let mut reverb = Reverb::new(44100);

    let controls = reverb.controls();
    assert_eq!(controls.len(), 7);
    assert_eq!(controls[0].key, "room_size");
    assert_eq!(controls[1].key, "decay");
    assert_eq!(controls[6].key, "freeze");

    reverb.set_control("room_size", 0.8).unwrap();
    assert!((reverb.get_control("room_size").unwrap() - 0.8).abs() < 1e-6);

    reverb.set_control("decay", 0.7).unwrap();
    assert!((reverb.get_control("decay").unwrap() - 0.7).abs() < 1e-6);

    reverb.set_control("freeze", 1.0).unwrap();
    assert!((reverb.get_control("freeze").unwrap() - 1.0).abs() < 1e-6);

    // As a number, freeze is on above one half only.
    for (value, frozen) in [(0.5, 0.0), (-1.0, 0.0), (0.51, 1.0), (0.0, 0.0)] {
        reverb.set_control("freeze", value).unwrap();
        assert_eq!(reverb.get_control("freeze").unwrap(), frozen, "{value}");
    }
}

#[test]
fn test_factory() {
    let factory = ReverbFactory;
    assert_eq!(ModuleFactory::type_id(&factory), "reverb");

    let config = serde_json::json!({
        "room_size": 0.7,
        "decay": 0.6,
        "damping": 0.4,
        "wet": 0.5,
        "dry": 0.8,
        "freeze": false
    });

    let result = factory.build(44100, &config).unwrap();

    let module = result.module.module();
    assert_eq!(module.name(), "Reverb");
    assert!(result.handles.is_empty());
    assert!(result.control_surface.is_some());
    assert!(result.sink.is_none());
}

/// A wet-only reverb fed a sustained 220 Hz sine, one sample at a time.
struct Sustained {
    reverb: Reverb,
    phase: f32,
    /// The last two outputs, oldest first.
    last: [f32; 2],
}

impl Sustained {
    const SAMPLE_RATE: u32 = 44100;

    /// Settled for two seconds at `room_size`.
    fn new(room_size: f32) -> Self {
        let mut reverb = Reverb::new(Self::SAMPLE_RATE);
        reverb.set_control("wet", 1.0).unwrap();
        reverb.set_control("dry", 0.0).unwrap();
        reverb.set_control("room_size", room_size).unwrap();
        let mut sustained = Self {
            reverb,
            phase: 0.0,
            last: [0.0; 2],
        };
        sustained.settle();
        sustained
    }

    fn settle(&mut self) {
        self.largest_kink(2 * Self::SAMPLE_RATE as usize);
    }

    /// The largest change of the left output's sample-to-sample step over
    /// `frames`. A sine's step changes slowly, by a thousandth of its
    /// amplitude per sample at 220 Hz; a jump changes it by the whole jump.
    fn largest_kink(&mut self, frames: usize) -> f32 {
        let mut largest = 0.0f32;
        for _ in 0..frames {
            let input = 0.5 * (self.phase * std::f32::consts::TAU).sin();
            self.phase += 220.0 / Self::SAMPLE_RATE as f32;
            self.phase -= self.phase.floor();
            self.reverb.set_input("audio_left", input).unwrap();
            self.reverb.process(1);
            let out = self.reverb.get_output("audio_left").unwrap();
            let [older, old] = self.last;
            largest = largest.max(((out - old) - (old - older)).abs());
            self.last = [old, out];
        }
        largest
    }

    fn set_room_size(&mut self, room_size: f32) {
        self.reverb.set_control("room_size", room_size).unwrap();
    }
}

/// A tenth of a second: longer than a crossfade.
const WINDOW: usize = 4410;

/// How far a change's kinks may exceed those of the sine settled at either
/// end. Sweeping the room moves the comb resonances, so the sine may swell
/// a little on the way (under twice as large); reading new offsets at once
/// kinks the output 25 to 250 times further.
const CLICK_RATIO: f32 = 4.0;

/// Asserts that `changing`, the largest kink during a change, is no
/// larger than the sine settled before the change and after it allows.
fn assert_no_click(case: &str, before: f32, changing: f32, sustained: &mut Sustained) {
    sustained.settle();
    let after = sustained.largest_kink(WINDOW);
    let limit = CLICK_RATIO * before.max(after);
    assert!(
        changing <= limit,
        "{case}: a kink of {changing} during the change, against {before} before \
         and {after} after"
    );
}

#[test]
fn room_size_step_does_not_click() {
    for (from, to) in [(0.2, 0.9), (0.9, 0.2), (0.3, 0.6)] {
        let mut sustained = Sustained::new(from);
        let before = sustained.largest_kink(WINDOW);
        sustained.set_room_size(to);
        let changing = sustained.largest_kink(WINDOW);
        assert_no_click(&format!("{from} to {to}"), before, changing, &mut sustained);
    }
}

#[test]
fn decay_step_does_not_click() {
    let mut sustained = Sustained::new(0.5);
    let before = sustained.largest_kink(WINDOW);
    sustained.reverb.set_control("decay", 0.95).unwrap();
    let changing = sustained.largest_kink(WINDOW);
    assert_no_click("decay 0.5 to 0.95", before, changing, &mut sustained);
}

#[test]
fn room_size_changes_mid_crossfade_do_not_click() {
    let mut sustained = Sustained::new(0.2);
    let before = sustained.largest_kink(WINDOW);
    let mut changing = 0.0f32;
    for room_size in [0.9, 0.4, 0.7] {
        sustained.set_room_size(room_size);
        changing = changing.max(sustained.largest_kink(300));
    }
    changing = changing.max(sustained.largest_kink(WINDOW));
    assert_no_click("retargeted mid-fade", before, changing, &mut sustained);
}

#[test]
fn automated_room_size_does_not_zipper() {
    let mut sustained = Sustained::new(0.2);
    let before = sustained.largest_kink(WINDOW);
    // A one-second ramp to 0.8, written every 32 samples.
    let writes = Sustained::SAMPLE_RATE as usize / 32;
    let mut changing = 0.0f32;
    for write in 1..=writes {
        sustained.set_room_size(0.2 + 0.6 * write as f32 / writes as f32);
        changing = changing.max(sustained.largest_kink(32));
    }
    changing = changing.max(sustained.largest_kink(WINDOW));
    assert_no_click("ramped", before, changing, &mut sustained);
}
