//! Edits that must keep working as before, or better: an audio unpatch does
//! not click, a connection patched back in plays its source again, and a
//! connected signal gains no latency.

use super::*;

const AUDIO_INTO_VCA: Edge = Edge("tone", "audio", "amp", "audio");
const AUDIO_INTO_DAC: Edge = Edge("tone", "audio", "dac", "audio");

/// A sine through a vca into the dac; `audio` unpatches the vca's input.
fn through_vca(connected: bool) -> Value {
    let mut connections = vec![wire("amp", "dac", "audio")];
    if connected {
        connections.push(AUDIO_INTO_VCA.json());
    }
    document(
        vec![
            tone("tone", json!({})),
            module("amp", "vca", json!({})),
            dac(),
        ],
        connections,
    )
}

/// A sine straight into the dac.
fn straight_to_dac(connected: bool) -> Value {
    let connections = if connected {
        vec![AUDIO_INTO_DAC.json()]
    } else {
        vec![]
    };
    document(vec![tone("tone", json!({})), dac()], connections)
}

type Build = fn(bool) -> Value;

/// (label, document, the edit that drops the audio, modes it applies to).
fn audio_cases() -> Vec<(&'static str, Build, Edge, Mode, Edit)> {
    let mut cases = Vec::new();
    for mode in [Mode::Offline, Mode::Live] {
        cases.push((
            "tone into vca: disconnect",
            through_vca as Build,
            AUDIO_INTO_VCA,
            mode,
            Edit::Disconnect(AUDIO_INTO_VCA),
        ));
        cases.push((
            "tone into vca: remove source",
            through_vca,
            AUDIO_INTO_VCA,
            mode,
            Edit::Remove("tone"),
        ));
        cases.push((
            "tone into dac: disconnect",
            straight_to_dac,
            AUDIO_INTO_DAC,
            mode,
            Edit::Disconnect(AUDIO_INTO_DAC),
        ));
    }
    cases.push((
        "tone into vca: reload",
        through_vca,
        AUDIO_INTO_VCA,
        Mode::Live,
        Edit::Reload(through_vca(false)),
    ));
    cases
}

/// "An audio unpatch must not click, so no step far beyond the steady
/// step": taken as at most 3x the sine's own steepest step, against the
/// 20+ times a cut at a peak makes.
const AUDIO_STEP_LIMIT: f32 = 3.0;

#[test]
fn an_audio_unpatch_does_not_click_and_falls_silent() {
    let mut cases = Cases::default();
    for (name, build, _, mode, edit) in audio_cases() {
        let label = format!("{name} / {mode:?}");
        let mut rig = Rig::start(mode, &build(true));
        let mut y = rig.render(BEFORE);
        rig.apply(&edit);
        y.extend(rig.render(AFTER_LONG));
        cases.check(
            format!("{label} (no click)"),
            steps_within(&y, BEFORE, AUDIO_STEP_LIMIT),
        );
        let tail = max_abs(&y[y.len() - 20 * MS..]);
        cases.check(
            format!("{label} (silent by the end)"),
            if tail <= 1e-4 {
                Ok(())
            } else {
                Err(format!("still sounding at {tail} 80 ms after the unpatch"))
            },
        );
    }
    cases.finish();
}

#[test]
fn a_connection_patched_back_in_plays_its_source_again() {
    let mut cases = Cases::default();
    let modulation: [(&str, Build, Edge); 3] = [
        (
            "vca level",
            crate::unpatch::vca_level,
            crate::unpatch::VCA_LEVEL,
        ),
        (
            "amplitude_mod",
            crate::unpatch::amplitude_mod,
            crate::unpatch::AMPLITUDE_MOD,
        ),
        (
            "cutoff_mod",
            crate::unpatch::cutoff_mod,
            crate::unpatch::CUTOFF_MOD,
        ),
    ];
    let audio: [(&str, Build, Edge); 2] = [
        ("audio into vca", through_vca, AUDIO_INTO_VCA),
        ("audio into dac", straight_to_dac, AUDIO_INTO_DAC),
    ];
    for mode in [Mode::Offline, Mode::Live] {
        for (name, build, edge) in modulation.into_iter().chain(audio) {
            let label = format!("{name} / {mode:?}");
            let whole = BEFORE + 2 * AFTER_LONG;
            let always = render_offline(&build(true), whole);
            let mut rig = Rig::start(mode, &build(true));
            let mut y = rig.render(BEFORE);
            rig.apply(&Edit::Disconnect(edge));
            y.extend(rig.render(AFTER_LONG));
            rig.apply(&Edit::Connect(edge));
            y.extend(rig.render(AFTER_LONG));
            let off = max_difference(&y, &always, whole - 20 * MS, whole);
            cases.check(
                format!("{label} (plays again)"),
                if y.iter().all(|v| v.is_finite()) && off <= 1e-3 {
                    Ok(())
                } else {
                    Err(format!(
                        "{off:.5} from the never-unpatched render by the end"
                    ))
                },
            );
        }
    }
    cases.finish();
}

/// The sine and a 100 Hz square, each alone into the dac.
fn alone(source: Value) -> Value {
    let id = source["id"].as_str().unwrap().to_string();
    document(vec![source, dac()], vec![wire(&id, "dac", "audio")])
}

fn square_100() -> Value {
    module(
        "sq",
        "oscillator",
        json!({ "waveform": "square", "frequency": 100.0 }),
    )
}

#[test]
fn a_connected_signal_gains_no_latency() {
    let frames = 4800;
    let sine = render_offline(&alone(tone("tone", json!({}))), frames);
    let square = render_offline(&alone(square_100()), frames);
    assert!(square.iter().any(|v| *v < 0.0) && square.iter().any(|v| *v > 0.0));

    // A vca's level follows the square (clamped to 0..1) sample for sample.
    let vca = document(
        vec![
            tone("tone", json!({})),
            square_100(),
            module("amp", "vca", json!({ "level": 0.25 })),
            dac(),
        ],
        vec![
            wire("tone", "amp", "audio"),
            wire("sq", "amp", "level"),
            wire("amp", "dac", "audio"),
        ],
    );
    let want_vca: Vec<f32> = sine
        .iter()
        .zip(&square)
        .map(|(s, q)| s * q.clamp(0.0, 1.0))
        .collect();

    // Full amplitude modulation: scale (am + 1) / 2, i.e. 1 on +1, 0 on -1.
    let am = document(
        vec![
            tone("tone", json!({ "amplitude_mod_depth": 1.0 })),
            square_100(),
            dac(),
        ],
        vec![
            wire("tone", "dac", "audio"),
            wire("sq", "tone", "amplitude_mod"),
        ],
    );
    let want_am: Vec<f32> = sine
        .iter()
        .zip(&square)
        .map(|(s, q)| s * (q + 1.0) * 0.5)
        .collect();

    let mut cases = Cases::default();
    for mode in [Mode::Offline, Mode::Live] {
        for (name, doc, want) in [
            ("vca level", &vca, &want_vca),
            ("amplitude_mod", &am, &want_am),
        ] {
            let y = Rig::start(mode, doc).render(frames);
            let off = max_difference(&y, want, 0, frames);
            cases.check(
                format!("{name} / {mode:?}"),
                if off <= 1e-6 {
                    Ok(())
                } else {
                    let at = (0..frames)
                        .find(|&i| (y[i] - want[i]).abs() > 1e-6)
                        .unwrap();
                    Err(format!(
                        "{off:.6} from the sample-for-sample result; first at {at}"
                    ))
                },
            );
        }
    }
    cases.finish();
}
