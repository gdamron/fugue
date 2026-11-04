use super::*;

const CURVES: [Curve; 4] = [Curve::Soft, Curve::Hard, Curve::Fold, Curve::Asymmetric];

fn sweep() -> impl Iterator<Item = f32> {
    (-4000..=4000).map(|n| n as f32 * 0.01)
}

#[test]
fn every_curve_passes_zero_with_unit_slope_and_stays_within_one() {
    for curve in CURVES {
        assert_eq!(curve.apply(0.0), 0.0, "{curve:?}");
        let slope = (curve.apply(1e-3) - curve.apply(-1e-3)) / 2e-3;
        assert!((slope - 1.0).abs() < 1e-2, "{curve:?} slope {slope}");
        for x in sweep() {
            let y = curve.apply(x);
            assert!(y.abs() <= 1.0, "{curve:?}({x}) = {y}");
        }
    }
}

#[test]
fn the_curves_have_their_shapes() {
    assert_eq!(Curve::Soft.apply(3.0), 1.0);
    assert_eq!(Curve::Soft.apply(-9.0), -1.0);
    assert!((Curve::Soft.apply(0.5) - 0.5f32.tanh()).abs() < 5e-3);
    assert_eq!(Curve::Hard.apply(2.0), 1.0);
    assert_eq!(Curve::Hard.apply(-0.25), -0.25);
    assert_eq!(Curve::Fold.apply(1.0), 1.0);
    assert!((Curve::Fold.apply(1.5) - 0.5).abs() < 1e-6);
    assert!(Curve::Fold.apply(2.0).abs() < 1e-6);
    assert!((Curve::Fold.apply(-3.0) - 1.0).abs() < 1e-6);
    assert_eq!(Curve::Asymmetric.apply(5.0), 1.0);
    assert_eq!(Curve::Asymmetric.apply(-5.0), -0.5);
}

#[test]
fn the_antiderivatives_integrate_their_curves() {
    for curve in [Curve::Hard, Curve::Fold] {
        let mut previous = curve.antiderivative(-6.0).unwrap();
        let mut x = -6.0f64;
        while x < 6.0 {
            let next = curve.antiderivative(x + 1e-3).unwrap();
            let slope = (next - previous) / 1e-3;
            let mid = curve.apply((x + 0.5e-3) as f32) as f64;
            assert!(
                (slope - mid).abs() < 2e-3,
                "{curve:?} at {x}: {slope} vs {mid}"
            );
            previous = next;
            x += 1e-3;
        }
        assert_eq!(curve.antiderivative(0.0), Some(0.0));
    }
}

#[test]
fn a_waveshaper_follows_its_curve_on_slow_signals_and_stays_bounded() {
    for curve in CURVES {
        let mut shaper = Waveshaper::new(curve);
        let mut last = 0.0;
        for x in sweep() {
            let y = shaper.process(x);
            assert!(y.abs() <= 1.0 + 1e-6, "{curve:?}({x}) = {y}");
            // Half a sample behind: within a step's change of the curve.
            let expected = curve.apply(x - 0.005);
            assert!(
                (y - expected).abs() < 2e-2,
                "{curve:?}({x}): {y} vs {expected}"
            );
            last = y;
        }
        assert!(last.is_finite());
        // Equal inputs fall back to the curve itself.
        shaper.rest_at(0.4);
        assert!((shaper.process(0.4) - curve.apply(0.4)).abs() < 1e-6);
        assert_eq!(shaper.curve(), curve);
    }
}
