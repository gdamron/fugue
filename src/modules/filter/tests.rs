//! The filter's controls, config and classic responses.

use super::*;
use crate::factory::ModuleFactory;

#[test]
fn test_filter_controls() {
    let mut filter = Filter::new(44100);

    // Test control metadata
    let controls = filter.controls();
    assert_eq!(controls.len(), 8);
    assert_eq!(controls[0].key, "cutoff");
    assert_eq!(controls[1].key, "resonance");
    assert_eq!(controls[2].key, "filter_type");

    // Test get/set controls
    filter.set_control("cutoff", 2000.0).unwrap();
    assert_eq!(filter.get_control("cutoff").unwrap(), 2000.0);

    filter.set_control("filter_type", 1.0).unwrap(); // HighPass
    assert_eq!(filter.get_control("filter_type").unwrap(), 1.0);
}

#[test]
fn test_filter_lowpass_attenuates_highs() {
    let mut filter = Filter::new(44100)
        .with_filter_type(FilterType::LowPass)
        .with_cutoff(100.0)
        .with_resonance(0.0);

    let mut output_sum = 0.0f32;
    for i in 0..1000 {
        filter
            .set_input("audio", if i % 2 == 0 { 1.0 } else { -1.0 })
            .unwrap();
        filter.process(1);
        output_sum += filter.get_output("audio").unwrap().abs();
    }

    let avg_output = output_sum / 1000.0;
    assert!(
        avg_output < 0.1,
        "Low-pass should attenuate high frequencies, got avg {}",
        avg_output
    );
}

#[test]
fn test_filter_factory() {
    let factory = FilterFactory;
    assert_eq!(ModuleFactory::type_id(&factory), "filter");

    let config = serde_json::json!({
        "filter_type": "highpass",
        "cutoff": 500.0,
        "resonance": 0.3
    });

    let result = factory.build(44100, &config).unwrap();

    let module = result.module.module();
    assert_eq!(module.name(), "Filter");

    assert!(result.control_surface.is_some());
}

#[test]
fn config_and_control_share_filter_type_and_modulation_names() {
    let factory = FilterFactory;
    // The control's spellings (`low_pass`) are config spellings too, so a
    // written control value rebuilds from the config.
    let config = serde_json::json!({
        "filter_type": "band_pass",
        "cutoff_mod_depth": 1234.0
    });
    let result = factory.build(44100, &config).unwrap();
    let surface = result.control_surface.unwrap();
    assert_eq!(
        surface.get_control("filter_type").unwrap(),
        "bandpass".into()
    );
    assert_eq!(
        surface.get_control("cutoff_mod_depth").unwrap(),
        1234.0.into()
    );
    assert!(surface.get_control("type").is_err());
    assert!(surface.get_control("cv_amount").is_err());

    let mut filter = Filter::new(44100);
    assert!(filter.set_input("cutoff_mod", 1.0).is_ok());
    assert!(filter.set_input("cutoff_cv", 1.0).is_err());
}

#[test]
fn every_filter_type_spelling_still_loads() {
    let registry = crate::ModuleRegistry::default();
    for (spelled, read) in [
        ("lowpass", "lowpass"),
        ("low_pass", "lowpass"),
        ("LPF", "lowpass"),
        ("low", "lowpass"),
        ("highpass", "highpass"),
        ("high_pass", "highpass"),
        ("hpf", "highpass"),
        ("High", "highpass"),
        ("bandpass", "bandpass"),
        ("band_pass", "bandpass"),
        ("bpf", "bandpass"),
        ("band", "bandpass"),
        ("bell", "bell"),
        ("peak", "bell"),
        ("lowshelf", "lowshelf"),
        ("low_shelf", "lowshelf"),
        ("high_shelf", "highshelf"),
        ("notch", "notch"),
        ("band_stop", "notch"),
    ] {
        let config = serde_json::json!({ "filter_type": spelled });
        let built = registry.build("filter", 48_000, &config).unwrap();
        let surface = built.control_surface.unwrap();
        assert_eq!(surface.get_control("filter_type").unwrap(), read.into());
        surface.set_control("filter_type", spelled.into()).unwrap();
        assert_eq!(surface.get_control("filter_type").unwrap(), read.into());
    }
    let config = serde_json::json!({ "filter_type": "allpass" });
    assert!(registry.build("filter", 48_000, &config).is_err());
    // Anything but text still leaves the default.
    for config in [
        serde_json::json!({ "filter_type": null }),
        serde_json::json!({ "filter_type": 2 }),
    ] {
        let built = registry.build("filter", 48_000, &config).unwrap();
        let surface = built.control_surface.unwrap();
        assert_eq!(
            surface.get_control("filter_type").unwrap(),
            "lowpass".into()
        );
    }
}

#[test]
fn there_is_no_type_alias_for_the_filter_type() {
    // Neither a document's config nor a control write has ever taken
    // `type` for `filter_type`; the declared table does not start to.
    let registry = crate::ModuleRegistry::default();
    let config = serde_json::json!({ "type": "highpass" });
    assert!(registry.build("filter", 48_000, &config).is_err());
    let built = registry
        .build("filter", 48_000, &serde_json::json!({}))
        .unwrap();
    let surface = built.control_surface.unwrap();
    assert!(surface.set_control("type", "highpass".into()).is_err());
}

#[test]
fn controls_clamp_as_they_always_did() {
    let mut filter = Filter::new(44100);
    filter.set_control("cutoff", 5.0).unwrap();
    assert_eq!(filter.get_control("cutoff").unwrap(), 20.0);
    filter.set_control("cutoff", 99999.0).unwrap();
    assert_eq!(filter.get_control("cutoff").unwrap(), 20000.0);
    filter.set_control("resonance", 3.0).unwrap();
    assert_eq!(filter.get_control("resonance").unwrap(), 1.0);
    filter.set_control("cutoff_mod_depth", -1.0).unwrap();
    assert_eq!(filter.get_control("cutoff_mod_depth").unwrap(), 0.0);
    assert!(filter.set_control("filter_type", 7.0).is_err());
    assert!(filter.set_control("cutoff", f32::NAN).is_err());
}
