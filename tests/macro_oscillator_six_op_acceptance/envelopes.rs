//! C9 (held versus released), C10 (morph stretches) and C11 (retrigger).

use super::rig::*;

const EDGE: usize = 960;

fn rms_over(x: &[f32], from: f64, to: f64) -> f32 {
    rms(&x[EDGE + seconds(from)..EDGE + seconds(to)])
}

/// Criterion C9: "at 220 Hz, for every patch, a note held for 1 s is at least
/// as loud 1.2 s after its trigger as the same note released after 0.2 s
/// (rms over 1.15-1.25 s); for at least 16 patches the held note is louder by
/// 6 dB or more."
/// Assumption: "at least as loud" allows 0.1% plus 1e-6 of slack for float
/// noise when both are silent.
#[test]
fn held_notes_outlast_released_notes() {
    let total = EDGE + seconds(1.3);
    let mut louder = 0;
    let mut table = Vec::new();
    for patch in 0..32 {
        let play_note = |release: f64| {
            let mut v = triggered(patch, &[]);
            let x = play(&mut v, &gate(total, &[(EDGE, EDGE + seconds(release))]));
            rms_over(&x, 1.15, 1.25)
        };
        let (held, released) = (play_note(1.0), play_note(0.2));
        assert!(
            held * 1.001 + 1e-6 >= released,
            "{} (patch {patch}): held rms {held} is below released rms {released} at 1.2 s",
            PATCH_NAMES[patch]
        );
        if held >= 2.0 * released {
            louder += 1;
        }
        table.push((patch, held, released));
    }
    assert!(
        louder >= 16,
        "the held note is 6 dB louder for only {louder} of 32 patches: {table:?}"
    );
}

/// Seconds from the envelope's peak to the last moment it is more than
/// 40 dB under the peak, or infinity if it never gets there. The envelope is
/// the peak magnitude per 5 ms window.
fn fall_time_40_db(x: &[f32]) -> f64 {
    let win = seconds(0.005);
    let env: Vec<f32> = x.chunks(win).map(peak).collect();
    let (top, max) = env
        .iter()
        .enumerate()
        .fold((0, 0.0f32), |a, (i, v)| if *v > a.1 { (i, *v) } else { a });
    assert!(max > 0.001, "note is silent");
    let threshold = max * 0.01;
    let last = env.iter().rposition(|v| *v > threshold).unwrap();
    if last + 1 >= env.len() {
        return f64::INFINITY;
    }
    (last - top) as f64 * 0.005
}

/// Criterion C10: "for patches 9 (glockenspiel), 11 (marimba) and 20 (harp),
/// triggered once at 220 Hz with `trigger` falling after 5 ms, the time for
/// `audio`'s envelope to fall 40 dB below its peak is longer at morph 0.8
/// than at morph 0.5, and longer at 0.5 than at 0.2."
/// Assumption: envelope is the peak magnitude per 5 ms window; the time runs
/// from the peak window to the last window more than 40 dB under it, over a
/// 15 s render; a note that has not fallen by then counts as infinite.
#[test]
fn morph_stretches_the_decay_of_struck_patches() {
    for patch in [9usize, 11, 20] {
        let time = |morph: f32| {
            let mut v = triggered(patch, &[("morph", morph)]);
            let t = fall_time_40_db(&play(
                &mut v,
                &gate(seconds(15.0), &[(EDGE, EDGE + seconds(0.005))]),
            ));
            t
        };
        let (short, normal, long) = (time(0.2), time(0.5), time(0.8));
        assert!(
            long > normal && normal > short,
            "{} (patch {patch}): 40 dB fall times {short} s (morph 0.2), {normal} s (0.5), \
             {long} s (0.8) are not increasing",
            PATCH_NAMES[patch]
        );
    }
}

/// Self-check of the measurement: an exponentially decaying sine falling
/// 60 dB in 1 s crosses -40 dB at about 0.667 s.
#[test]
fn fall_time_measurement_reads_a_known_decay() {
    let x: Vec<f32> = (0..seconds(3.0))
        .map(|i| {
            let t = i as f64 / SRF;
            (10f64.powf(-3.0 * t) * (std::f64::consts::TAU * 220.0 * t).sin()) as f32
        })
        .collect();
    let t = fall_time_40_db(&x);
    assert!((t - 0.667).abs() < 0.03, "read {t} s");
}

const PULSE: usize = 2400;
const PERIOD: usize = 7200;

fn retrigger_gate() -> Vec<f32> {
    let edges: Vec<(usize, usize)> = (0..)
        .map(|k| 480 + k * PERIOD)
        .take_while(|e| e + PULSE < seconds(2.0))
        .map(|e| (e, e + PULSE))
        .collect();
    gate(seconds(2.0), &edges)
}

/// The largest step of one untouched note of `patch` (pulse of 50 ms).
fn single_note_step(patch: usize) -> f32 {
    let mut v = triggered(patch, &[]);
    max_step(&play(&mut v, &gate(seconds(2.0), &[(480, 480 + PULSE)])))
}

/// Criterion C11: "triggering patch 10 (vibraphone) at 220 Hz every 150 ms
/// for 2 s, no sample-to-sample step exceeds 1.5x the largest step of a
/// single untouched note of the same patch. Patch changes between triggers
/// (writing `harmonics` between notes) obey the same bound against the larger
/// of the two patches' single-note steps."
/// Assumptions: each trigger pulse is 50 ms high; the first edge is at
/// 10 ms; a patch change is a `harmonics` write 100 ms after each rising edge
/// (during the previous note's tail, before the next edge), alternating
/// between the two patches; pairs 10/11, 10/20 and 10/26 are tested.
#[test]
fn retriggering_is_click_free() {
    let mut v = triggered(10, &[]);
    let x = play(&mut v, &retrigger_gate());
    assert!(peak(&x) > 0.01, "retriggered notes are silent");
    let bound = 1.5 * single_note_step(10);
    assert!(
        max_step(&x) <= bound,
        "retrigger step {} exceeds 1.5x a single note ({bound})",
        max_step(&x)
    );
}

#[test]
fn patch_changes_between_triggers_are_click_free() {
    for other in [11usize, 20, 26] {
        let bound = 1.5 * single_note_step(10).max(single_note_step(other));
        let mut v = triggered(10, &[]);
        let gate = retrigger_gate();
        let (x, _) = drive(&mut v, &gate, |v, frame| {
            // The 100 ms mark after a rising edge, aligned to a block start.
            let since = frame.wrapping_sub(480) % PERIOD;
            if frame >= 480 && since / HOST_BLOCK == 4800 / HOST_BLOCK {
                let note = (frame - 480) / PERIOD;
                let patch = if note % 2 == 0 { other } else { 10 };
                v.set("harmonics", patch_harmonics(patch));
            }
        });
        assert!(peak(&x) > 0.01, "10/{other}: silent");
        assert!(
            max_step(&x) <= bound,
            "patch 10 with {other}: step {} exceeds 1.5x a single note ({bound})",
            max_step(&x)
        );
    }
}
