use super::*;
use crate::dsp::BlockRamp;
use crate::modules::macro_oscillator::engines::wave_shapes::wave_index;

/// The power of harmonic `n` of one cycle of `table`.
fn harmonic_power(table: &[f32], n: usize) -> f64 {
    let size = table.len() as f64;
    let (mut re, mut im) = (0.0, 0.0);
    for (k, &v) in table.iter().enumerate() {
        let angle = TAU * (n * k) as f64 / size;
        re += v as f64 * angle.cos();
        im += v as f64 * angle.sin();
    }
    (re * re + im * im) / (size * size)
}

#[test]
fn inverse_fft_turns_one_harmonic_into_a_cosine() {
    let mut re = vec![0.0; 16];
    let mut im = vec![0.0; 16];
    re[3] = 1.0;
    inverse_transform(&mut re, &mut im);
    for (k, v) in re.iter().enumerate() {
        let expected = (TAU * 3.0 * k as f64 / 16.0).cos();
        assert!((v - expected).abs() < 1e-12, "{v} vs {expected} at {k}");
    }
}

#[test]
fn levels_halve_their_harmonics_every_octave() {
    let kept: Vec<usize> = (0..LEVELS).map(harmonics).collect();
    assert_eq!(kept, [128, 90, 64, 45, 32, 22, 16, 11, 8, 5, 4, 2, 2, 1, 1]);
}

#[test]
fn every_level_stays_below_nyquist_where_it_is_read() {
    // Across the whole pitch range, the brighter of the two levels read
    // keeps only harmonics below half the sample rate.
    let mut frequency = 1e-4f32;
    while frequency <= 0.25 {
        let position = level_position(frequency);
        let level = (position as usize).min(LEVELS - 2);
        let top = harmonics(level) as f32 * frequency;
        assert!(top <= 0.5 + 1e-4, "{top} at {frequency} (level {level})");
        frequency *= 1.01;
    }
}

#[test]
fn each_level_holds_only_the_harmonics_it_keeps() {
    let tables = wavetables();
    // The brightest saw-like wave of the first bank.
    let wave = wave_index(0, 0, 7);
    for level in [0, 4, 9, 13] {
        let table = tables.table(wave, level);
        let kept = harmonics(level);
        let inside: f64 = (1..=kept).map(|n| harmonic_power(table, n)).sum();
        let outside: f64 = (kept + 1..table.len() / 2)
            .map(|n| harmonic_power(table, n))
            .sum();
        assert!(inside > 0.0);
        assert!(
            outside < inside * 1e-9,
            "level {level}: {outside} vs {inside}"
        );
    }
}

#[test]
fn every_wave_is_normalised_and_wraps() {
    let tables = wavetables();
    for wave in 0..WAVES {
        let table = tables.table(wave, 0);
        let peak = table.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let rms = (table.iter().map(|v| v * v).sum::<f32>() / table.len() as f32).sqrt();
        assert!(peak <= PEAK as f32 + 1e-4, "wave {wave}: peak {peak}");
        assert!(rms <= RMS as f32 + 1e-4, "wave {wave}: rms {rms}");
        assert!(rms > 0.1, "wave {wave}: rms {rms}");
        for level in 1..LEVELS {
            let peak = tables
                .table(wave, level)
                .iter()
                .fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(
                peak <= PEAK as f32 + 1e-4,
                "wave {wave} level {level}: peak {peak}"
            );
        }
        // The repeated sample closes the cycle for interpolation.
        let start = wave * tables.stride + tables.offsets[0];
        assert_eq!(tables.samples[start + tables.sizes[0]], table[0]);
    }
}

/// A fingerprint of every sample of every wave of `bank` at every level,
/// to four decimal places: a hash, so a change to any wave shows.
fn bank_digest(tables: &Wavetables, bank: usize) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for wave in bank * SIDE * SIDE..(bank + 1) * SIDE * SIDE {
        for level in 0..LEVELS {
            for &v in tables.table(wave, level) {
                for byte in ((v * 1e4).round() as i32).to_le_bytes() {
                    hash = (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3);
                }
            }
        }
    }
    hash
}

#[test]
fn regenerating_the_tables_gives_the_same_waves() {
    // The generator is the source of the tables: these fingerprints pin
    // every sample it makes, so a change to it is a deliberate one.
    let tables = wavetables();
    let digests: Vec<u64> = (0..BANKS).map(|bank| bank_digest(tables, bank)).collect();
    assert_eq!(digests, PINNED, "{digests:#x?}");
}

const PINNED: [u64; BANKS] = [
    0x44b8_a1a8_e411_3303,
    0xa89f_2093_bd96_67d3,
    0xbcee_c83d_dd09_57fc,
    0x3219_b9df_6297_a6d1,
];

#[test]
fn the_level_never_reads_brighter_than_the_pitch_allows() {
    // Jumps up and down by up to six octaves within one block, then holds:
    // at every sample, the brighter level read keeps its harmonics below
    // Nyquist at that sample's pitch.
    let jumps = [
        (0.002, 0.04),
        (0.04, 0.002),
        (0.01, 0.011),
        (0.2, 0.003),
        (0.003, 0.2),
    ];
    for (from, to) in jumps {
        let mut frequency = BlockRamp::new(from);
        let mut level = BlockRamp::new(level_position(from));
        for _ in 0..3 {
            let target = follow_pitch(&mut level, frequency.value(), to);
            let fs = frequency.steps(to, 12);
            for (f, position) in fs.zip(level.steps(target, 12)) {
                let bright = (position as usize).min(LEVELS - 2);
                let top = harmonics(bright) as f32 * f;
                assert!(top <= 0.5 + 1e-4, "{from} -> {to}: {top} at {f}");
            }
        }
    }
}

#[test]
fn reading_between_levels_crossfades() {
    let tables = wavetables();
    let wave = wave_index(0, 0, 7);
    let bright = tables.read(wave, &tables.locate(0.3, 2.0));
    let dark = tables.read(wave, &tables.locate(0.3, 3.0));
    let between = tables.read(wave, &tables.locate(0.3, 2.5));
    assert!((between - 0.5 * (bright + dark)).abs() < 1e-6);
}

/// The power at non-harmonic frequencies below `top`, relative to the
/// power at the harmonics, of `signal` at fundamental `frequency` (cycles
/// per sample), measured with a Hann-windowed DFT.
fn inharmonic_ratio(signal: &[f32], frequency: f64, top: f64) -> f64 {
    let n = signal.len();
    let window: Vec<f64> = (0..n)
        .map(|k| 0.5 - 0.5 * (TAU * k as f64 / n as f64).cos())
        .collect();
    let bins = (top * n as f64) as usize;
    let (mut harmonic, mut other) = (0.0, 0.0);
    for bin in 1..bins {
        let (mut re, mut im) = (0.0, 0.0);
        for (k, (&v, w)) in signal.iter().zip(&window).enumerate() {
            let angle = TAU * (bin * k) as f64 / n as f64;
            re += v as f64 * w * angle.cos();
            im += v as f64 * w * angle.sin();
        }
        let power = re * re + im * im;
        let cycles = bin as f64 / (frequency * n as f64);
        if (cycles - cycles.round()).abs() * frequency * n as f64 <= 3.0 && cycles.round() >= 1.0 {
            harmonic += power;
        } else {
            other += power;
        }
    }
    other / harmonic
}

#[test]
fn the_mip_read_aliases_far_less_than_a_naive_read() {
    let tables = wavetables();
    let wave = wave_index(0, 0, 7);
    let frequency = 1.0 / 23.5;
    let level = level_position(frequency as f32);
    let (mut mip, mut naive) = (Vec::new(), Vec::new());
    let mut phase = 0.0f32;
    for _ in 0..4096 {
        mip.push(tables.read(wave, &tables.locate(phase, level)));
        naive.push(tables.read(wave, &tables.locate(phase, 0.0)));
        phase += frequency as f32;
        phase -= phase.floor();
    }
    let mip = inharmonic_ratio(&mip, frequency, 20.0 / 48.0);
    let naive = inharmonic_ratio(&naive, frequency, 20.0 / 48.0);
    let db = |r: f64| 10.0 * r.log10();
    assert!(db(mip) < -40.0, "mip read: {} dB", db(mip));
    assert!(
        db(naive) > db(mip) + 20.0,
        "naive {} dB vs mip {} dB",
        db(naive),
        db(mip)
    );
}
