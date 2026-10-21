//! What the table's tests hold threads up with: hooks where another
//! thread's change can land, and writes held up mid-way.

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::modules::melody::MelodyControls;

/// What runs on this thread at a later call of [`between_loads`]: how many
/// calls to let pass first, and what.
type Hook = (usize, Box<dyn FnOnce()>);

thread_local! {
    static BETWEEN_LOADS: RefCell<Option<Hook>> = RefCell::new(None);
}

/// Called by the table where another thread's change can land: between a
/// degrees' header load and a weights' generation load, between a weights'
/// generation load and a count load, after a position write lands and
/// before it bumps the version, and after a copy reads each position.
pub(in super::super) fn between_loads() {
    let run = BETWEEN_LOADS.with(|hook| {
        let mut hook = hook.borrow_mut();
        match hook.as_mut() {
            Some((pass, _)) if *pass > 0 => {
                *pass -= 1;
                None
            }
            _ => hook.take().map(|(_, run)| run),
        }
    });
    if let Some(run) = run {
        run();
    }
}

/// Runs `edit` the next time this thread calls [`between_loads`].
pub(super) fn between_the_next_loads(edit: impl FnOnce() + 'static) {
    between_loads_after(0, edit);
}

/// Runs `edit` once this thread has let `pass` calls of [`between_loads`]
/// go by.
pub(super) fn between_loads_after(pass: usize, edit: impl FnOnce() + 'static) {
    BETWEEN_LOADS.with(|hook| *hook.borrow_mut() = Some((pass, Box::new(edit))));
}

/// Waits (up to ten seconds) until `progress` moves `by` past where it was.
pub(super) fn wait_for(progress: &AtomicUsize, by: usize) {
    let (from, deadline) = (progress.load(Ordering::Acquire), Instant::now());
    while progress.load(Ordering::Acquire) < from + by {
        assert!(deadline.elapsed() < Duration::from_secs(10), "no progress");
        std::thread::yield_now();
    }
}

/// Ten seconds: long enough for any thread to get going, short enough that
/// a regression fails rather than hangs.
pub(super) const PATIENCE: Duration = Duration::from_secs(10);

/// Writes `value` to degree position `index` on another thread, held up
/// after its header load until `release` sends or drops: returns the
/// write, already holding its generation, and `release`.
pub(super) fn held_write(
    controls: &MelodyControls,
    index: usize,
    value: i32,
) -> (std::thread::JoinHandle<bool>, mpsc::Sender<()>) {
    let (release, held) = mpsc::channel::<()>();
    let (holding, holds) = mpsc::channel();
    let controls = controls.clone();
    let write = std::thread::spawn(move || {
        between_the_next_loads(move || {
            holding.send(()).unwrap();
            let _ = held.recv_timeout(PATIENCE);
        });
        controls.table.write_degree(index, value)
    });
    holds.recv_timeout(PATIENCE).expect("the write is held up");
    (write, release)
}

/// Writes `value` to degree position `index` on another thread, held up
/// after it lands and before it finishes, until `release` sends or drops:
/// returns the write and `release`.
pub(super) fn landed_write(
    controls: &MelodyControls,
    index: usize,
    value: i32,
) -> (std::thread::JoinHandle<bool>, mpsc::Sender<()>) {
    let (release, held) = mpsc::channel::<()>();
    let (holding, holds) = mpsc::channel();
    let controls = controls.clone();
    let write = std::thread::spawn(move || {
        // Past the header load, to the landing.
        between_loads_after(1, move || {
            holding.send(()).unwrap();
            let _ = held.recv_timeout(PATIENCE);
        });
        controls.table.write_degree(index, value)
    });
    holds.recv_timeout(PATIENCE).expect("the write lands");
    (write, release)
}

/// Waits (up to [`PATIENCE`]) until `thread` finishes, and joins it.
pub(super) fn join_soon<T>(thread: std::thread::JoinHandle<T>) -> T {
    let deadline = Instant::now() + PATIENCE;
    while !thread.is_finished() {
        assert!(Instant::now() < deadline, "the thread never finished");
        std::thread::yield_now();
    }
    thread.join().unwrap()
}
