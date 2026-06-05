use super::*;

#[test]
fn test_silence_in_silence_out() {
    let mut reverb = Reverb::new(44100);
    for _ in 0..2000 {
        reverb.process(1);
        let l = reverb.get_output("left").unwrap();
        let r = reverb.get_output("right").unwrap();
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
    reverb.set_input("left", 1.0).unwrap();
    reverb.process(1);
    reverb.set_input("left", 0.0).unwrap();

    // Check for non-zero output in the tail
    let mut found_output = false;
    for _ in 0..8000 {
        reverb.process(1);
        let l = reverb.get_output("left").unwrap();
        let r = reverb.get_output("right").unwrap();
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

    reverb.set_input("left", 0.75).unwrap();
    reverb.set_input("right", -0.5).unwrap();
    reverb.process(1);

    let l = reverb.get_output("left").unwrap();
    let r = reverb.get_output("right").unwrap();
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
        reverb.set_input("left", 0.5).unwrap();
        reverb.process(1);
    }
    reverb.set_input("left", 0.0).unwrap();

    // Enable freeze
    reverb.set_control("freeze", 1.0).unwrap();

    // Output should remain non-zero (frozen feedback)
    let mut energy = 0.0f32;
    for _ in 0..4000 {
        reverb.process(1);
        energy += reverb.get_output("left").unwrap().abs();
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
        reverb.set_input("left", 0.1).unwrap();
        reverb.process(1);
    }
    reverb.set_input("left", 0.0).unwrap();

    // Run many samples of silence — output should stay bounded
    for i in 0..50000 {
        reverb.process(1);
        let l = reverb.get_output("left").unwrap();
        let r = reverb.get_output("right").unwrap();
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
    assert_eq!(result.handles.len(), 1);
    assert_eq!(result.handles[0].0, "controls");
    assert!(result.control_surface.is_some());
    assert!(result.sink.is_none());
}
