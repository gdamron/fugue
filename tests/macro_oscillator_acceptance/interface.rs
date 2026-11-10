//! Every declared control accepts its whole range.

use super::rig::*;
use super::*;

/// Criterion: every declared control accepts both ends of its declared range
/// (numbers min and max, choices every option), including `key` and `scale`
/// when the chords engine is present, and the module keeps producing
/// bounded audio afterwards.
#[test]
fn every_declared_control_accepts_its_whole_range() {
    let mut v = voice(json!({}));
    for meta in v.controls.clone() {
        match meta.kind {
            ControlKind::Number { min, max, .. } => {
                for value in [min, max] {
                    v.set(&meta.key, value);
                    let got = v.module.module().get_control(&meta.key).unwrap();
                    assert!(
                        (got - value).abs() <= 1e-3 * value.abs().max(1.0),
                        "{} set to {value} reads {got}",
                        meta.key
                    );
                }
            }
            ControlKind::String { options: Some(o) } => {
                for position in 0..o.len() {
                    v.set(&meta.key, position as f32);
                }
            }
            _ => {}
        }
        let x = v.render(BLOCK);
        assert!(
            x.iter().all(|s| s.is_finite() && s.abs() <= 1.0),
            "{} range",
            meta.key
        );
    }
}
