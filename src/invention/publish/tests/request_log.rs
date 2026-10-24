//! The session's request log: every applied request with the sample it
//! applied at, in apply order, recorded on a control thread from the
//! outcomes the audio thread already sends.

use super::declared::dial_rig;
use super::migrated::block_with_locks_held;
use super::request_beats::beat_rig;
use super::requests::counted_block;
use super::*;
use crate::control_request::{
    BeatSpec, BeatTime, ControlIndex, Request, RequestId, RequestValue, RtValue, When,
};
use crate::{LoggedBeat, LoggedRequest, LoggedValue, RequestLog};

impl LiveGraph {
    /// Submits `value` for declared control `key` of `module_id` at `when`
    /// as a front door would, logging it with `beat` as its musical time.
    /// For tests, until a front door submits timed requests (FUG-317).
    pub(crate) fn submit_logged(
        &self,
        (module_id, key): (&str, &str),
        value: crate::control_request::RtValue,
        when: crate::control_request::When,
        beat: Option<crate::LoggedBeat>,
    ) -> RequestId {
        let user = crate::control_request::Source::User;
        self.submit_logged_from((module_id, key), value, when, beat, user)
    }

    /// [`Self::submit_logged`], from `source`.
    pub(crate) fn submit_logged_from(
        &self,
        (module_id, key): (&str, &str),
        value: crate::control_request::RtValue,
        when: crate::control_request::When,
        beat: Option<crate::LoggedBeat>,
        source: crate::control_request::Source,
    ) -> RequestId {
        use crate::control_request::{Request, RequestValue, RtValue};
        let surface = self.control_surfaces.lock().unwrap()[module_id].clone();
        let declaration = surface.declaration(key).unwrap();
        let publisher = self.publisher.lock().unwrap();
        let target = publisher
            .control_target(module_id, declaration.index)
            .unwrap();
        let mut request = Request::new(target, RequestValue::Value(value));
        request.when = when;
        request.source = source;
        request.event = declaration.decl.event;
        let mut pending = self.pending.lock().unwrap();
        let id = self.requests.submit(request).unwrap();
        let shown = match value {
            RtValue::F32(number) => crate::ControlValue::Number(number),
            RtValue::I32(number) => crate::ControlValue::Number(number as f32),
            RtValue::U32(number) => crate::ControlValue::Number(number as f32),
            RtValue::Bool(flag) => crate::ControlValue::Bool(flag),
        };
        let write = PendingWrite {
            module_id: module_id.to_string(),
            key: key.to_string(),
            value: shown,
            raw: Some(value),
            beat,
            automated: source == crate::control_request::Source::Automation,
            announced: false,
            awaited: false,
            twin: None,
            authored: None,
        };
        pending.submitted(id, write);
        id
    }
}

/// Submits `value` for `key` of `module_id` at `when`, logged.
fn submit_logged(
    rig: &Rig,
    target: (&str, &str),
    value: RtValue,
    when: When,
    beat: Option<LoggedBeat>,
) -> RequestId {
    rig.live.submit_logged(target, value, when, beat)
}

/// The clock's position in the installed order, for a beat request.
fn clock_idx(rig: &Rig) -> u32 {
    let publisher = rig.live.publisher().lock().unwrap();
    publisher
        .control_target("clock", ControlIndex(0))
        .unwrap()
        .module_idx as u32
}

fn logged(sample: u64, key: &str, value: LoggedValue) -> LoggedRequest {
    LoggedRequest {
        sample,
        module_id: "dial".to_string(),
        key: key.to_string(),
        value,
        due: None,
        beat: None,
    }
}

#[test]
fn applied_writes_are_logged_with_their_sample_in_apply_order() {
    let mut rig = dial_rig();
    let dial = rig.live.control_surfaces.lock().unwrap()["dial"].clone();
    // Applied at the next block start: sample 64.
    dial.set_control("level", 0.5.into()).unwrap();
    dial.set_control("shape", "steep".into()).unwrap();
    rig.render(1);
    // Later in the block it is submitted for, then before an earlier one.
    submit_logged(
        &rig,
        ("dial", "level"),
        RtValue::F32(0.75),
        When::AtSample(200),
        None,
    );
    submit_logged(
        &rig,
        ("dial", "level"),
        RtValue::F32(0.125),
        When::AtSample(150),
        None,
    );
    // Superseded before it applies: never heard, never logged.
    submit_logged(
        &rig,
        ("dial", "pulse"),
        RtValue::Bool(true),
        When::AtSample(300),
        None,
    );
    submit_logged(
        &rig,
        ("dial", "level"),
        RtValue::F32(0.9),
        When::AtSample(300),
        None,
    );
    submit_logged(
        &rig,
        ("dial", "level"),
        RtValue::F32(0.3),
        When::AtSample(300),
        None,
    );
    rig.render(4);

    let log = rig.live.request_log();
    assert_eq!(
        log.requests,
        [
            logged(64, "level", LoggedValue::Number(0.5)),
            logged(64, "shape", LoggedValue::Index(1)),
            logged(150, "level", LoggedValue::Number(0.125)),
            logged(200, "level", LoggedValue::Number(0.75)),
            logged(300, "pulse", LoggedValue::Bool(true)),
            logged(300, "level", LoggedValue::Number(0.3)),
        ]
    );
    assert_eq!((log.full, log.lost), (false, 0));
    assert_eq!(log.edits, 1, "the rig's own edit, adding the dial");
    assert!(!log.is_exact(), "an edited session replays inexactly");
}

#[test]
fn a_late_request_logs_where_it_applied_and_when_it_was_due() {
    let mut rig = dial_rig();
    rig.render(4);
    submit_logged(
        &rig,
        ("dial", "level"),
        RtValue::F32(0.5),
        When::AtSample(10),
        None,
    );
    rig.render(1);
    let log = rig.live.request_log();
    assert_eq!(
        log.requests,
        [LoggedRequest {
            due: Some(10),
            ..logged(320, "level", LoggedValue::Number(0.5))
        }]
    );
}

#[test]
fn a_beat_request_logs_its_sample_and_musical_time() {
    let mut rig = beat_rig(120.0);
    rig.render(1);
    let clock = clock_idx(&rig);
    let beat = LoggedBeat::After {
        clock: "clock".to_string(),
        beats: 1.0,
    };
    let when = When::Beat(BeatTime {
        clock,
        spec: BeatSpec::After(1.0),
        ramp: None,
    });
    submit_logged(
        &rig,
        ("dial", "level"),
        RtValue::F32(0.5),
        when,
        Some(beat.clone()),
    );
    // A beat at 120 bpm is 24000 samples.
    rig.render(24_000 / 64 + 2);
    let log = rig.live.request_log();
    assert_eq!(log.requests.len(), 1, "{log:?}");
    let applied = &log.requests[0];
    assert_eq!(applied.beat, Some(beat));
    assert!(
        (24_000..=24_064 + 64).contains(&applied.sample),
        "one beat after it was taken: {}",
        applied.sample
    );
}

#[test]
fn a_repeating_grid_makes_the_log_inexact() {
    let mut rig = beat_rig(120.0);
    rig.render(1);
    let clock = clock_idx(&rig);
    let spec = BeatSpec::Grid {
        every: 1.0,
        offset: 0.0,
        repeat: true,
    };
    let beat = LoggedBeat::Grid {
        clock: "clock".to_string(),
        every: 1.0,
        offset: 0.0,
        repeat: true,
    };
    let when = When::Beat(BeatTime {
        clock,
        spec,
        ramp: None,
    });
    submit_logged(&rig, ("dial", "level"), RtValue::F32(0.5), when, Some(beat));
    rig.render(24_000 / 64 * 3);
    let log = rig.live.request_log();
    assert_eq!(log.requests.len(), 1, "its first application only");
    assert!(!RequestLog { edits: 0, ..log }.is_exact());
}

#[test]
fn outcomes_the_audio_thread_dropped_are_counted_lost() {
    let mut rig = dial_rig();
    // More applications than the outcome ring holds, with no control
    // thread receiving in between. Events, so none coalesce.
    for _ in 0..6 {
        for _ in 0..200 {
            submit_logged(
                &rig,
                ("dial", "pulse"),
                RtValue::Bool(true),
                When::Now,
                None,
            );
        }
        rig.render(1);
    }
    let log = rig.live.request_log();
    assert!(log.lost > 0, "{} recorded", log.requests.len());
    assert_eq!(log.requests.len() as u64 + log.lost, 1200);
    assert!(!log.is_exact());
}

#[test]
fn a_write_the_log_no_longer_remembers_is_counted_lost() {
    let mut rig = dial_rig();
    let publisher = rig.live.publisher().lock().unwrap();
    let target = publisher.control_target("dial", ControlIndex(0)).unwrap();
    rig.live
        .requests
        .submit(Request::new(target, RequestValue::Value(RtValue::F32(0.5))))
        .unwrap();
    drop(publisher);
    rig.render(1);
    let log = rig.live.request_log();
    assert_eq!((log.requests.len(), log.lost), (0, 1));
}

#[test]
fn blocks_applying_logged_requests_neither_allocate_nor_lock() {
    let mut rig = dial_rig();
    for n in 0..32 {
        let at = When::AtSample(64 * 2 + n * 3);
        submit_logged(&rig, ("dial", "level"), RtValue::F32(0.5), at, None);
    }
    rig.render(1);
    assert_eq!(counted_block(&mut rig), (0, 0));
    // With the pending log's lock held too (the negative control, a block
    // that does take a held lock, is `migrated`'s).
    assert_eq!(block_with_locks_held(&mut rig, &[]), Ok((0, 0)));
    assert_eq!(rig.live.request_log().requests.len(), 32);
}

#[test]
fn the_log_stops_when_full_and_says_so() {
    let mut recorder = crate::invention::request_log::Recorder::default();
    for sample in 0..=crate::REQUEST_LOG_CAPACITY as u64 {
        recorder.applied(LoggedRequest {
            sample,
            ..logged(0, "level", LoggedValue::Number(0.5))
        });
    }
    let log = recorder.export();
    assert!(log.full);
    assert_eq!(log.requests.len(), crate::REQUEST_LOG_CAPACITY);
    assert_eq!(log.requests[0].sample, 0, "the oldest are kept");
    assert!(!log.is_exact());
}

#[test]
fn the_exported_log_round_trips_through_json() {
    let log = RequestLog {
        requests: vec![
            LoggedRequest {
                due: Some(3),
                ..logged(64, "level", LoggedValue::Number(0.1))
            },
            LoggedRequest {
                beat: Some(LoggedBeat::Grid {
                    clock: "clock".into(),
                    every: 0.5,
                    offset: 0.25,
                    repeat: false,
                }),
                ..logged(96, "shape", LoggedValue::Index(1))
            },
            logged(97, "seed", LoggedValue::Integer(-(1 << 30) - 1)),
        ],
        ..RequestLog::default()
    };
    let json = serde_json::to_string(&log).unwrap();
    assert_eq!(serde_json::from_str::<RequestLog>(&json).unwrap(), log);
    assert!(log.is_exact());
}

#[test]
fn a_log_with_timed_edits_round_trips_and_one_from_before_them_still_reads() {
    let edit = crate::LoggedEdit {
        when: crate::LoggedWhen::AtSample { sample: 640 },
        outcome: crate::LoggedEditOutcome::Landed {
            sample: 640,
            due: None,
        },
        removed: vec!["osc".into()],
        built: vec![crate::LoggedModule {
            id: "osc2".into(),
            module_type: "oscillator".into(),
            config: serde_json::json!({ "waveform": "square" }),
            writes: vec![crate::LoggedSetting {
                key: "frequency".into(),
                value: crate::ControlValue::Number(220.0),
            }],
        }],
        connections: vec![crate::LoggedConnection {
            from: "osc2".into(),
            from_port: "audio".into(),
            to: "dac".into(),
            to_port: "audio".into(),
        }],
        requests_before: 0,
    };
    let log = RequestLog {
        timed_edits: vec![edit],
        ..RequestLog::default()
    };
    let json = serde_json::to_string(&log).unwrap();
    assert_eq!(serde_json::from_str::<RequestLog>(&json).unwrap(), log);
    assert!(log.is_exact());
    // Written before timed edits were logged: each counted as a commit, so
    // it reads, and stays inexact.
    let old = r#"{ "sample_rate": 48000, "block_size": 64, "requests": [], "edits": 1 }"#;
    let old: RequestLog = serde_json::from_str(old).unwrap();
    assert!(old.timed_edits.is_empty());
    assert!(!old.is_exact());
}

/// A running session of the step sequencer example on a manual backend.
fn running_example() -> (
    crate::RunningInvention,
    crate::invention::manual_backend::Pump,
) {
    use crate::invention::manual_backend::{start_manual, SAMPLE_RATE};
    let json = include_str!("../../../../examples/step_sequencer.json");
    let document = Invention::from_json(json).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(document).unwrap();
    start_manual(runtime)
}

#[test]
fn a_session_log_names_its_rate_and_block_size_and_starts_exact() {
    let (running, pump) = running_example();
    pump.render(4);
    let log = running.request_log();
    assert_eq!((log.sample_rate, log.block_size), (48_000, 64));
    assert_eq!((log.edits, log.unlogged, log.actors), (0, 0, 0));
    assert!(log.is_exact());
    running.stop();
}

#[test]
fn writes_that_reach_a_module_without_a_request_make_the_log_inexact() {
    let (running, pump) = running_example();
    let snapshot = running.snapshot();
    // Declared: a request, logged.
    snapshot
        .set_control("osc", "waveform", "square".into())
        .unwrap();
    pump.render(1);
    let log = running.request_log();
    assert_eq!((log.requests.len(), log.unlogged), (1, 0));
    // An input write: straight to the module.
    running.set_module_input("osc", "frequency", 220.0).unwrap();
    pump.render(1);
    let log = running.request_log();
    assert_eq!(log.unlogged, 1);
    assert!(!log.is_exact());
    running.stop();
}

#[test]
fn every_commit_counts_as_an_edit_even_one_publishing_nothing() {
    let mut rig = dial_rig();
    let before = rig.live.request_log().edits;
    rig.adopt_registry();
    rig.render(1);
    assert_eq!(rig.live.request_log().edits, before + 1);
}

#[test]
fn code_and_agent_modules_make_the_log_inexact() {
    let (running, pump) = running_example();
    pump.render(1);
    // Standing in for a code module's entry: building one needs its
    // script runtime, and only its type is counted.
    let module = crate::RuntimeModuleInfo {
        id: "script".into(),
        module_type: "code".into(),
        config: serde_json::json!({}),
    };
    let mut state = running.state.lock().unwrap();
    state.modules.insert("script".into(), module);
    drop(state);
    let log = running.request_log();
    assert_eq!(log.actors, 1);
    assert!(!log.is_exact());
    running.stop();
}
