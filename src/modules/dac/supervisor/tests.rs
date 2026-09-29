use super::*;
use cpal::SupportedBufferSize;
use std::sync::atomic::AtomicU64;

fn range(channels: u16, min: u32, max: u32, format: SampleFormat) -> SupportedStreamConfigRange {
    SupportedStreamConfigRange::new(channels, min, max, SupportedBufferSize::Unknown, format)
}

fn default_config() -> SupportedStreamConfig {
    SupportedStreamConfig::new(2, 44_100, SupportedBufferSize::Unknown, SampleFormat::F32)
}

#[test]
fn backoff_doubles_up_to_the_cap_and_resets() {
    let mut backoff = Backoff::default();
    assert!(backoff.is_first_failure());
    let delays: Vec<_> = (0..7).map(|_| backoff.failed()).collect();
    assert_eq!(delays[0], Duration::from_millis(100));
    assert_eq!(delays[1], Duration::from_millis(200));
    assert_eq!(delays[4], Duration::from_millis(1600));
    assert_eq!(delays[5], MAX_RETRY);
    assert_eq!(delays[6], MAX_RETRY);
    assert!(!backoff.is_first_failure());

    backoff.reset();
    assert_eq!(backoff.failed(), FIRST_RETRY);
}

#[test]
fn backoff_never_overflows() {
    let mut backoff = Backoff { failures: u32::MAX };
    assert_eq!(backoff.failed(), MAX_RETRY);
}

#[test]
fn rebuild_config_prefers_the_default_format_and_channels() {
    let ranges = vec![
        range(1, 8_000, 96_000, SampleFormat::F32),
        range(2, 8_000, 96_000, SampleFormat::I16),
        range(2, 8_000, 96_000, SampleFormat::F32),
    ];
    let config = config_at_rate(&default_config(), ranges, 48_000).unwrap();
    assert_eq!(config.sample_rate(), 48_000);
    assert_eq!(config.channels(), 2);
    assert_eq!(config.sample_format(), SampleFormat::F32);
}

#[test]
fn rebuild_config_falls_back_to_matching_channels_then_anything() {
    let ranges = vec![
        range(1, 8_000, 96_000, SampleFormat::F32),
        range(2, 8_000, 96_000, SampleFormat::I16),
    ];
    let config = config_at_rate(&default_config(), ranges, 48_000).unwrap();
    assert_eq!(config.channels(), 2);
    assert_eq!(config.sample_format(), SampleFormat::I16);

    let ranges = vec![range(6, 8_000, 96_000, SampleFormat::I32)];
    let config = config_at_rate(&default_config(), ranges, 48_000).unwrap();
    assert_eq!(config.channels(), 6);
}

#[test]
fn rebuild_config_refuses_a_rate_no_range_supports() {
    let ranges = vec![range(2, 44_100, 44_100, SampleFormat::F32)];
    assert!(config_at_rate(&default_config(), ranges, 48_000).is_none());
}

#[test]
fn only_stopping_errors_request_a_rebuild() {
    use super::super::StreamErrorKind;
    assert!(StreamErrorKind::DeviceNotAvailable.stops_stream());
    assert!(StreamErrorKind::StreamInvalidated.stops_stream());
    assert!(!StreamErrorKind::DeviceChanged.stops_stream());
    assert!(!StreamErrorKind::RealtimeDenied.stops_stream());
    assert!(!StreamErrorKind::Other.stops_stream());
}

fn test_shared() -> Arc<Shared> {
    Arc::new(Shared {
        render: Mutex::new(Box::new(|_: &mut [f32], _: &mut [f32]| {})),
        diagnostics: Arc::new(AudioDiagnostics::new()),
        sample_rate: 48_000,
        log_missed_deadlines: false,
        rebuild_requested: AtomicBool::new(false),
        shutdown: AtomicBool::new(false),
        supervisor: OnceLock::new(),
    })
}

/// Runs `supervise` on its own thread with a fake opener and reports
/// whether it exited within `timeout`.
fn exits_within(
    shared: Arc<Shared>,
    open: impl FnMut(&Arc<Shared>) -> Result<(), Box<dyn std::error::Error>> + Send + 'static,
    timeout: Duration,
) -> bool {
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (done_tx, done_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        supervise(shared, ready_tx, open);
        let _ = done_tx.send(());
    });
    ready_rx.recv().unwrap().unwrap();
    let exited = done_rx.recv_timeout(timeout).is_ok();
    if exited {
        handle.join().unwrap();
    }
    exited
}

/// `stop()` lands while a rebuild is inside cpal, whose own parking eats
/// the wake. The supervisor must still exit instead of sleeping forever.
#[test]
fn stop_during_a_rebuild_is_not_lost() {
    let shared = test_shared();
    let mut opens = 0;
    let open = move |shared: &Arc<Shared>| {
        opens += 1;
        if opens == 2 {
            // Like `Supervisor::stop` arriving mid-rebuild...
            shared.shutdown.store(true, Ordering::Release);
            shared.wake();
            // ...and cpal parking internally, consuming that wake.
            thread::park_timeout(Duration::from_millis(50));
        }
        Ok(())
    };
    let requester = shared.clone();
    let trigger = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        requester.request_rebuild();
    });
    assert!(
        exits_within(shared, open, Duration::from_secs(2)),
        "supervisor hung after a stop during a rebuild"
    );
    trigger.join().unwrap();
}

/// A rebuild requested while another rebuild is inside cpal still runs.
#[test]
fn a_rebuild_requested_mid_rebuild_is_not_lost() {
    let shared = test_shared();
    let opened = Arc::new(AtomicU64::new(0));
    let count = opened.clone();
    let open = move |shared: &Arc<Shared>| {
        let n = count.fetch_add(1, Ordering::SeqCst) + 1;
        if n == 2 {
            shared.request_rebuild();
            thread::park_timeout(Duration::from_millis(50));
        }
        Ok(())
    };
    let requester = shared.clone();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let handle = thread::spawn(move || supervise(shared.clone(), ready_tx, open));
    ready_rx.recv().unwrap().unwrap();
    requester.request_rebuild();

    let deadline = Instant::now() + Duration::from_secs(2);
    while opened.load(Ordering::SeqCst) < 3 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(opened.load(Ordering::SeqCst), 3, "second rebuild never ran");
    assert_eq!(requester.diagnostics.snapshot().stream_restart_count, 2);

    requester.shutdown.store(true, Ordering::Release);
    requester.wake();
    handle.join().unwrap();
}

/// Stopping while retrying for a device does not wait out the backoff.
#[test]
fn stop_while_waiting_for_a_device_exits_promptly() {
    let shared = test_shared();
    let mut opens = 0;
    let open = move |_: &Arc<Shared>| {
        opens += 1;
        if opens == 1 {
            Ok(())
        } else {
            Err("no output device".into())
        }
    };
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let thread_shared = shared.clone();
    let handle = thread::spawn(move || supervise(thread_shared, ready_tx, open));
    ready_rx.recv().unwrap().unwrap();
    shared.request_rebuild();
    // Let a few failed retries accumulate a long backoff.
    thread::sleep(Duration::from_millis(400));

    let stopping = Instant::now();
    shared.shutdown.store(true, Ordering::Release);
    shared.wake();
    handle.join().unwrap();
    assert!(stopping.elapsed() < Duration::from_millis(500));
}

/// Exercises a real rebuild on the machine's default output. Plays
/// silence. Run with `cargo test -- --ignored` on a machine with audio.
#[test]
#[ignore = "needs an audio output device"]
fn a_requested_rebuild_keeps_the_same_render_function_playing() {
    let sample_rate = super::super::default_sample_rate().expect("an output device");
    let diagnostics = Arc::new(AudioDiagnostics::new());
    let rendered = Arc::new(AtomicU64::new(0));
    let counter = rendered.clone();
    let render: BlockRenderFn = Box::new(move |left, right| {
        counter.fetch_add(left.len() as u64, Ordering::Relaxed);
        left.fill(0.0);
        right.fill(0.0);
    });
    let mut supervisor =
        Supervisor::start(render, diagnostics.clone(), sample_rate, false).unwrap();

    thread::sleep(Duration::from_millis(300));
    let before = rendered.load(Ordering::Relaxed);
    assert!(before > 0, "the first stream rendered nothing");

    supervisor.shared.request_rebuild();
    thread::sleep(Duration::from_millis(500));

    let snapshot = diagnostics.snapshot();
    assert_eq!(snapshot.stream_restart_count, 1);
    assert!(
        rendered.load(Ordering::Relaxed) > before,
        "no audio after the rebuild"
    );
    assert!(snapshot.last_callback_age_ms.unwrap() < 100.0);

    supervisor.stop();
    let stopped_at = rendered.load(Ordering::Relaxed);
    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        rendered.load(Ordering::Relaxed),
        stopped_at,
        "audio after stop"
    );
}
