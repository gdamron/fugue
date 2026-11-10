//! Criterion: an invention that exceeds 1.0 renders offline with every
//! sample in [-1, 1].

use super::support::*;

fn assert_in_range(out: &[f32], what: &str) {
    for (i, x) in out.iter().enumerate() {
        assert!(x.is_finite(), "{what}: sample {i} is {x}");
        assert!((-1.0..=1.0).contains(x), "{what}: sample {i} is {x}");
    }
}

#[test]
fn the_clamp_holds_for_any_buffer_size() {
    let json = tone("sine", 333.0, 18.0);
    let whole = offline(&json, 6_000, 6_000);
    assert_in_range(&whole, "one buffer");
    for chunk in [1, 7, 64, 100, 513, 5_999] {
        assert_eq!(
            hash(&offline(&json, 6_000, chunk)),
            hash(&whole),
            "buffer size {chunk}"
        );
    }
}
