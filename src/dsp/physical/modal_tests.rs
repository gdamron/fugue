use super::*;

const RATE: u32 = 48_000;

fn settings(hz: f32, damping: f32) -> ModalSettings {
    ModalSettings {
        frequency: hz / RATE as f32,
        structure: 0.25,
        brightness: 0.5,
        damping,
        // The pickup at the middle hears no even-numbered mode.
        position: 0.5,
    }
}

/// Peak of `seconds` of ringing from `voice` with no input.
fn ring(voice: &mut ModalVoice, settings: &ModalSettings, seconds: f32) -> f32 {
    let silence = [0.0f32; 24];
    let mut peak = 0.0f32;
    for _ in 0..(seconds * RATE as f32 / 24.0) as usize {
        let (mut odd, mut even) = ([0.0f32; 24], [0.0f32; 24]);
        voice.process(settings, &silence, &mut odd, &mut even);
        for i in 0..24 {
            peak = peak.max((odd[i] + even[i]).abs());
        }
    }
    peak
}

/// The fundamental falls 60 dB in the stated time (the upper partials die
/// sooner), to within a few dB.
#[test]
fn the_body_falls_sixty_db_in_the_stated_time() {
    for damping in [0.3f32, 0.6] {
        let s = settings(220.0, damping);
        let mut voice = ModalVoice::new(RATE);
        // The fundamental and a second mode the pickup does not hear.
        voice.set_resolution(2);
        let mut impulse = [0.0f32; 24];
        impulse[0] = 1.0;
        voice.process(&s, &impulse, &mut [0.0; 24], &mut [0.0; 24]);
        let t60 = t60_seconds(damping);
        ring(&mut voice, &s, 0.05 * t60);
        let early = ring(&mut voice, &s, 0.1 * t60);
        ring(&mut voice, &s, 0.9 * t60);
        let late = ring(&mut voice, &s, 0.1 * t60);
        let fall = 20.0 * (early / late).log10();
        assert!(
            (54.0..66.0).contains(&fall),
            "damping {damping}: fell {fall} dB"
        );
    }
}

/// A smaller resolution feeds fewer modes but lets the ones already
/// ringing beyond it ring out, rather than cutting them.
#[test]
fn lowering_the_resolution_lets_upper_modes_ring_out() {
    let s = settings(110.0, 0.8);
    let mut voice = ModalVoice::new(RATE);
    let mut impulse = [0.0f32; 24];
    impulse[0] = 1.0;
    voice.process(&s, &impulse, &mut [0.0; 24], &mut [0.0; 24]);
    voice.set_resolution(12);
    voice.process(&s, &[0.0; 24], &mut [0.0; 24], &mut [0.0; 24]);
    assert_eq!(voice.running, MAX_MODES);
    let upper = voice.groups[GROUPS - 1].first.iter().any(|x| *x != 0.0);
    assert!(upper, "the top modes still ring");
    voice.reset();
    assert_eq!(ring(&mut voice, &s, 0.01), 0.0);
}

#[test]
fn stiffness_is_harmonic_between_a_quarter_and_three_tenths() {
    assert_eq!(stiffness(0.27), 0.0);
    assert!(stiffness(0.1) < 0.0);
    assert!(stiffness(0.6) > 0.0);
    assert!(stiffness(1.0) > stiffness(0.95));
}
