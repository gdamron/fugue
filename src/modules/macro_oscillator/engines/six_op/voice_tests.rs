//! One FM voice on its own: notes, releases, chokes and drones.

use super::patches::PATCHES;
use super::voice::{FmVoice, Play};

const SR: f32 = 48_000.0;
const BLOCK: usize = 12;

fn play(scrub: Option<f32>) -> Play {
    Play {
        note: 60.0,
        sample_period: 1.0 / SR,
        timbre: 0.5,
        morph: 0.5,
        scrub,
    }
}

fn render(voice: &mut FmVoice, play: &Play, seconds: f32) -> Vec<f32> {
    let blocks = (seconds * SR) as usize / BLOCK;
    let mut all = Vec::with_capacity(blocks * BLOCK);
    for _ in 0..blocks {
        let mut out = [0.0; BLOCK];
        voice.render(&PATCHES, play, &mut out);
        all.extend_from_slice(&out);
    }
    all
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

#[test]
fn a_new_voice_is_silent() {
    let mut voice = FmVoice::new();
    assert!(render(&mut voice, &play(None), 0.05)
        .iter()
        .all(|v| *v == 0.0));
    assert!(!voice.is_sounding());
}

#[test]
fn a_released_note_dies_away_and_the_voice_goes_quiet() {
    let mut voice = FmVoice::new();
    voice.note_on(&PATCHES, 0, 60.0, 0.8);
    let note = render(&mut voice, &play(None), 0.1);
    assert!(rms(&note) > 0.01);
    voice.note_off();
    render(&mut voice, &play(None), 8.0);
    assert!(!voice.is_sounding());
    assert!(render(&mut voice, &play(None), 0.05)
        .iter()
        .all(|v| *v == 0.0));
}

#[test]
fn a_different_patch_on_a_sounding_voice_waits_for_a_short_choke() {
    let mut voice = FmVoice::new();
    voice.note_on(&PATCHES, 26, 60.0, 0.8);
    render(&mut voice, &play(None), 0.3);
    voice.note_on(&PATCHES, 11, 60.0, 0.8);
    assert!(voice.is_fading(), "the change waits");
    assert_eq!(voice.patch(), 26, "still the old patch while it chokes");
    render(&mut voice, &play(None), 0.003);
    assert_eq!(voice.patch(), 11, "the new note starts once silent");
    let after = render(&mut voice, &play(None), 0.05);
    assert!(rms(&after) > 0.01);
}

#[test]
fn the_same_patch_retriggers_without_waiting() {
    let mut voice = FmVoice::new();
    voice.note_on(&PATCHES, 10, 60.0, 0.8);
    render(&mut voice, &play(None), 0.2);
    voice.note_on(&PATCHES, 10, 64.0, 0.8);
    assert!(!voice.is_fading());
}

#[test]
fn a_drone_reads_the_envelopes_where_the_scrub_points() {
    let level = |t: f32| {
        let mut voice = FmVoice::new();
        voice.drone(&PATCHES, 0, 0.0);
        let x = render(&mut voice, &play(Some(t)), 0.3);
        rms(&x[x.len() / 2..])
    };
    let onset = level(0.0);
    let later = level(0.8);
    assert!(onset > 0.01, "{onset}");
    assert!(later < 0.5 * onset, "{later} vs {onset}");
}

#[test]
fn fading_out_silences_in_the_time_asked() {
    let mut voice = FmVoice::new();
    voice.drone(&PATCHES, 26, 0.0);
    render(&mut voice, &play(Some(0.5)), 0.1);
    voice.fade_out(0.02);
    render(&mut voice, &play(Some(0.5)), 0.021);
    let x = render(&mut voice, &play(Some(0.5)), 0.05);
    assert!(x.iter().all(|v| *v == 0.0));
}

#[test]
fn a_drone_starts_at_its_level_rather_than_climbing_there() {
    let mut voice = FmVoice::new();
    voice.drone(&PATCHES, 24, 0.0);
    let x = render(&mut voice, &play(Some(0.5)), 0.3);
    let first = rms(&x[..480]);
    let steady = rms(&x[x.len() - 4800..]);
    assert!(first > 0.5 * steady, "{first} vs {steady}");
}

#[test]
fn a_high_drone_reads_no_further_than_the_patch_allows() {
    // Rate scaling runs a high note's envelopes faster; scrubbed to the
    // end, the drone still holds where the patch's range ends rather
    // than falling silent.
    let level = |note: f32| {
        let mut voice = FmVoice::new();
        voice.drone(&PATCHES, 0, 0.0);
        let play = Play {
            note,
            ..play(Some(1.5))
        };
        let x = render(&mut voice, &play, 0.3);
        rms(&x[x.len() / 2..])
    };
    assert!(level(96.0) > 0.001, "{}", level(96.0));
}
