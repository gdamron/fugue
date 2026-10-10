//! The audio side of a link never waits for a control-side peer preempted
//! mid-operation, and no audio-thread source uses std's channels, whose
//! `try_send` and `try_recv` can spin or yield then.

use std::path::Path;
use std::thread;

use super::*;

#[test]
fn a_reclaimer_preempted_mid_pop_never_holds_the_block() {
    let mut graph = base_graph();
    let ends = link(&mut graph, 1);
    let survivors = || {
        publication(
            vec![
                ("osc1", Next::Survivor("oscillator")),
                ("osc2", Next::Survivor("oscillator")),
                ("dac", Next::Survivor("dac")),
            ],
            vec![edge("osc1", "dac", "audio"), edge("osc2", "dac", "audio")],
        )
    };
    ends.publish(survivors());
    assert_eq!(counted_block(&mut graph), (0, 0));

    // The reclaimer has taken the first retirement but not released its
    // slot. A block installing the next publication finds the ring full at
    // once: it holds the retirement and finishes, clean.
    ends.publish(survivors());
    let reclaimed = ends.retired.pop_paused(|| {
        thread::scope(|scope| {
            scope.spawn(|| assert_eq!(counted_block(&mut graph), (0, 0)));
        });
    });
    assert!(reclaimed.is_some());
    assert_eq!(ends.applied.load(Ordering::Relaxed), 2);

    // Once the slot is released, the next block sends the held retirement.
    assert_eq!(counted_block(&mut graph), (0, 0));
    assert!(ends.retired.pop().is_some());
}

#[test]
fn a_writer_preempted_mid_push_never_holds_the_block() {
    let mut graph = base_graph();
    let mut ends = link(&mut graph, 4);
    let port = frequency_port(&graph);
    let write = |value| InputWrite {
        generation: 0,
        module_idx: 0,
        port_idx: port,
        value,
    };
    ends.inputs.push(write(0.25)).unwrap();

    // The writer has written its second value but not published it: the
    // block applies the first and finds the queue empty at once, clean.
    let pushed = ends.inputs.push_paused(write(0.5), || {
        thread::scope(|scope| {
            scope.spawn(|| assert_eq!(counted_block(&mut graph), (0, 0)));
        });
    });
    assert!(pushed.is_ok());
    assert!(osc1_frequency(&mut graph).iter().all(|v| *v == 0.25));

    // Once published, the next block applies it.
    render(&mut graph, 1);
    assert!(osc1_frequency(&mut graph).iter().all(|v| *v == 0.5));
}

/// Audio-thread sources: the graph and its link, the request queue, the
/// rings and payloads, and every module.
const AUDIO_SOURCES: [&str; 6] = [
    "src/invention/graph",
    "src/control_request",
    "src/modules",
    "src/spsc.rs",
    "src/payload.rs",
    "src/audio_thread.rs",
];

/// Files that may use std's channels, and why.
const ALLOWED: &[(&str, &str)] = &[(
    "src/modules/dac/supervisor.rs",
    "the stream's startup handshake, on the supervisor thread before any callback",
)];

/// Uses of std's channels in the code (not comments) of the non-test
/// files under `path`. A file's tests start at its inline `#[cfg(test)]`
/// module, as rustfmt places it.
fn scan(root: &Path, path: &Path, found: &mut Vec<String>) {
    let name = path.file_name().unwrap().to_string_lossy();
    if path.is_dir() {
        if name != "tests" {
            for entry in std::fs::read_dir(path).unwrap() {
                scan(root, &entry.unwrap().path(), found);
            }
        }
        return;
    }
    // `/`-separated on every platform, to match `ALLOWED`.
    let shown = path
        .strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    if name.ends_with("tests.rs")
        || !name.ends_with(".rs")
        || ALLOWED.iter().any(|(file, _)| *file == shown)
    {
        return;
    }
    let source = std::fs::read_to_string(path).unwrap();
    for (index, line) in source.lines().enumerate() {
        if line.trim() == "#[cfg(test)]"
            && source.lines().nth(index + 1).is_some_and(|next| {
                next.trim_start().starts_with("mod ") && next.trim_end().ends_with('{')
            })
        {
            break;
        }
        let code = line.split("//").next().unwrap();
        if code.contains("mpsc") || code.contains("sync_channel") {
            found.push(format!("{shown}:{}: {}", index + 1, line.trim()));
        }
    }
}

#[test]
fn no_audio_thread_source_uses_std_channels() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut found = Vec::new();
    for source in AUDIO_SOURCES {
        let path = root.join(source);
        assert!(path.exists(), "{source} moved: update this scan");
        scan(root, &path, &mut found);
    }
    assert!(
        found.is_empty(),
        "use crate::spsc on audio-thread paths, not std's channels:\n{}",
        found.join("\n")
    );
}
