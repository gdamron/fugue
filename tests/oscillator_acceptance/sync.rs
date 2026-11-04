use super::{alias_power, db, N, SKIP};
use fugue::dsp::{BandLimitedOscillator, Shape};

/// Runs a master at `m` and a saw slave at `s` (both phase increments), hard
/// synced via the master's `wrapped()`. With `synced` false the slave free-runs.
fn slave(m: f32, s: f32, len: usize, synced: bool) -> Vec<f32> {
    let (mut master, mut slave) = (BandLimitedOscillator::new(), BandLimitedOscillator::new());
    (0..len)
        .map(|_| {
            master.next(Shape::Saw, m, 0.5);
            match master.wrapped() {
                Some(since) if synced => slave.next_synced(Shape::Saw, s, 0.5, since),
                _ => slave.next(Shape::Saw, s, 0.5),
            }
        })
        .collect()
}

/// A master of exactly 64 samples per cycle makes the slave repeat every 64.
#[test]
fn slave_repeats_with_the_masters_period() {
    let max_diff = |y: &[f32]| {
        (640..2400)
            .map(|n| (y[n + 64] - y[n]).abs())
            .fold(0.0, f32::max)
    };
    // Control: unsynced, the slave (4.68 cycles per master cycle) does not repeat.
    assert!(max_diff(&slave(1.0 / 64.0, 0.0731, 2560, false)) > 0.1);
    let diff = max_diff(&slave(1.0 / 64.0, 0.0731, 2560, true));
    assert!(
        diff < 1e-3,
        "synced slave differs from itself one period later by {diff}"
    );
}

/// Hard sync aliases less than a naive synced saw. The 10 dB bound is the
/// tester's own choice, not from the spec.
#[test]
fn synced_saw_aliases_less_than_naive() {
    let (m, s) = (1.0 / 48.37, 2.6 / 48.37); // master ~992 Hz, slave ~2.58 kHz
    let (mut pm, mut ps) = (0.0f64, 0.0f64);
    let naive: Vec<f32> = (0..SKIP + N)
        .map(|_| {
            pm += m;
            if pm >= 1.0 {
                pm -= 1.0;
                ps = pm / m * s; // the slave restarted pm/m samples ago
            } else {
                ps += s;
            }
            ps -= ps.floor();
            (2.0 * ps - 1.0) as f32
        })
        .collect();
    let blep = slave(m as f32, s as f32, SKIP + N, true);
    let f0 = 48_000.0 * m;
    let ((a_blep, t_blep), (a_naive, t_naive)) = (alias_power(&blep, f0), alias_power(&naive, f0));
    println!("synced saw: {:.1} dB below naive", db(a_naive / a_blep));
    assert!(db(a_naive / a_blep) >= 10.0);
    assert!(db(t_blep / t_naive).abs() < 3.0, "total power is off");
}
