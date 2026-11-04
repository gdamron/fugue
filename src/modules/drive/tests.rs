use super::*;

const RATE: u32 = 48_000;

fn build(config: serde_json::Value) -> ModuleBuildResult {
    DriveFactory.build(RATE, &config).unwrap()
}

#[test]
fn the_factory_reads_controls_and_oversampling() {
    let config =
        serde_json::json!({ "shape": "foldback", "drive": 0.4, "oversampling": 2, "mix": 0.5 });
    let mut built = build(config);
    let module = built.module.module_mut();
    assert_eq!(module.inputs(), &["audio"]);
    assert_eq!(module.outputs(), &["audio"]);
    assert_eq!(module.get_control("shape").unwrap(), 2.0);
    assert_eq!(module.get_control("drive").unwrap(), 0.4);
    assert_eq!(module.get_control("mix").unwrap(), 0.5);
    assert_eq!(module.get_control("auto_level").unwrap(), 1.0);
    assert!(DriveFactory
        .build(RATE, &serde_json::json!({ "oversampling": 8 }))
        .is_err());
    assert!(DriveFactory
        .build(RATE, &serde_json::json!({ "shape": "crunchy" }))
        .is_err());
}

#[test]
fn oversampling_sets_the_latency_and_three_rounds_up_to_four() {
    let latency = |oversampling: u32| {
        let drive = Drive::with_cells(
            RATE,
            oversampling_factor(oversampling),
            Arc::new(ControlCells::new(controls::defaults())),
        );
        drive.latency()
    };
    assert_eq!(latency(1), 0);
    assert!(latency(2) > 0);
    assert_eq!(latency(3), latency(4));
    assert!(latency(4) > latency(2));
}

#[test]
fn defaults_suit_a_building_block_fully_wet() {
    let drive = Drive::new(RATE, Some(Factor::Four));
    assert_eq!(drive.get_control("mix").unwrap(), 1.0);
    assert_eq!(drive.get_control("level").unwrap(), 1.0);
    assert_eq!(drive.get_control("shape").unwrap(), 0.0);
    assert_eq!(drive.get_control("drive").unwrap(), 0.25);
}

#[test]
fn a_live_write_reaches_the_sound_and_out_of_range_values_clamp() {
    let mut drive = Drive::new(RATE, Some(Factor::Four));
    drive.set_control("drive", 7.0).unwrap();
    assert_eq!(drive.get_control("drive").unwrap(), 1.0);
    drive.set_control("bias", -3.0).unwrap();
    assert_eq!(drive.get_control("bias").unwrap(), -1.0);
    drive.set_control("level", 0.0).unwrap();
    drive.set_input("audio", 0.5).unwrap();
    for _ in 0..8 {
        drive.process(MAX_BLOCK);
    }
    assert_eq!(drive.get_output("audio").unwrap(), 0.0);
    assert!(drive.set_input("cv", 0.0).is_err());
}
