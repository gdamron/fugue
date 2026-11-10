//! Interface behavior: config starting values, every control across its
//! range, `level` 0, and the internal exciter switching off with `audio`.

use super::rig::*;
use super::*;

/// Criterion: "every config key is a control's starting value": config sets
/// the starting `frequency`, `damping`, `polyphony` and the model by name.
/// Assumption: `model` takes the option's name as a string in config, as
/// choice controls elsewhere in the library do.
#[test]
fn config_sets_starting_values() {
    let names = reso(json!({})).models;
    let last = names.len() - 1;
    let mut r = reso(json!({
        "model": names[last],
        "frequency": 330.0,
        "damping": 0.8,
        "polyphony": 2,
    }));
    let m = r.module.module();
    assert_eq!(m.get_control("model"), Ok(last as f32));
    assert_eq!(m.get_control("frequency"), Ok(330.0));
    assert_eq!(m.get_control("damping"), Ok(0.8));
    assert_eq!(m.get_control("polyphony"), Ok(2.0));
    r.strike();
    let x = r.render(seconds(0.5));
    assert!(
        peak(&x) > 1e-3,
        "sounds on the configured model {}",
        names[last]
    );
}

/// Criterion: every declared control accepts both ends of its declared range
/// (numbers min and max, choices every option) and the module keeps
/// producing bounded audio afterwards.
#[test]
fn every_declared_control_accepts_its_whole_range() {
    let mut r = reso(json!({}));
    for meta in r.controls.clone() {
        match meta.kind {
            ControlKind::Number { min, max, .. } => {
                for value in [min, max] {
                    r.set(&meta.key, value);
                    let got = r.module.module().get_control(&meta.key).unwrap();
                    assert!(
                        (got - value).abs() <= 1e-3 * value.abs().max(1.0),
                        "{} set to {value} reads {got}",
                        meta.key
                    );
                }
            }
            ControlKind::String { options: Some(o) } => {
                for position in 0..o.len() {
                    r.set(&meta.key, position as f32);
                }
            }
            _ => {}
        }
        r.strike();
        let x = r.render(BLOCK * 2);
        assert!(
            x.iter().all(|s| s.is_finite() && s.abs() <= 1.0),
            "{} range",
            meta.key
        );
    }
}

/// Criterion (INTERFACE.md): `level` scales the output and is smoothed, so
/// `level` 0 is silent once the glide has finished: below -80 dBFS on
/// `audio`, `odd` and `even` for a struck voice of every model.
/// Assumption: 0.3 s is long enough for the glide to finish.
#[test]
fn level_zero_is_silent() {
    let names = reso(json!({})).models;
    for (position, model) in names.iter().enumerate() {
        let mut r = ready(position, 220.0, 0.9);
        r.set("level", 0.0);
        r.render(seconds(0.3));
        r.strike();
        let out = r.render_all(seconds(1.0), BLOCK);
        for (name, x) in out.each() {
            assert!(
                peak(x) < 1e-4,
                "{model}: {name} peaks at {} with level 0",
                peak(x)
            );
        }
    }
}

/// Criterion: "unconnected `audio`: internal exciter on strum; connected
/// `audio`: internal exciter off": a strum with `audio` connected and silent
/// produces (almost) no sound, while the same strum with `audio` unconnected
/// does.
#[test]
fn connecting_audio_turns_the_internal_exciter_off() {
    let names = reso(json!({})).models;
    for (position, model) in names.iter().enumerate() {
        let mut open = ready(position, 220.0, 0.5);
        open.strike();
        assert!(
            peak(&open.render(seconds(0.5))) > 1e-3,
            "{model}: internal exciter silent"
        );

        let mut patched = ready(position, 220.0, 0.5);
        patched.connect_audio();
        patched.render(BLOCK);
        patched.strike();
        let x = patched.render(seconds(0.5));
        assert!(
            peak(&x) < 1e-4,
            "{model}: strum sounds with audio connected ({})",
            peak(&x)
        );
    }
}
