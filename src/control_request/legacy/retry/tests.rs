use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

use super::*;
use crate::alloc_counter::allocator_events;
use crate::{ControlMeta, ControlValue};

/// A legacy surface whose lock a test holds (`busy`), logging what lands:
/// `level` and `tone` values, `note_on` and `note_off` events.
#[derive(Default)]
struct Voice {
    busy: AtomicBool,
    log: [AtomicU32; 32],
    landed: AtomicUsize,
}

const KEYS: [&str; 4] = ["level", "tone", "note_on", "note_off"];

impl Voice {
    /// What landed, in order: (key, value).
    fn landed(&self) -> Vec<(&'static str, f32)> {
        (0..self.landed.load(Ordering::Relaxed))
            .map(|i| {
                let bits = self.log[i].load(Ordering::Relaxed);
                (KEYS[(bits >> 24) as usize], (bits & 0xff_ffff) as f32)
            })
            .collect()
    }
}

impl ControlSurface for Voice {
    fn controls(&self) -> Vec<ControlMeta> {
        Vec::new()
    }

    fn get_control(&self, _key: &str) -> Result<ControlValue, String> {
        Ok(ControlValue::Number(0.0))
    }

    fn set_control(&self, _key: &str, _value: ControlValue) -> Result<(), String> {
        Ok(())
    }

    fn set_scheduled(&self, key: &str, value: TableValue) -> Result<(), LegacyRefusal> {
        let key = KEYS.iter().position(|known| *known == key);
        let (Some(key), TableValue::Number(value)) = (key, value) else {
            return Err(LegacyRefusal::Unsupported);
        };
        if self.busy.load(Ordering::Relaxed) {
            return Err(LegacyRefusal::Busy);
        }
        let at = self.landed.fetch_add(1, Ordering::Relaxed);
        self.log[at].store((key as u32) << 24 | value as u32, Ordering::Relaxed);
        Ok(())
    }

    fn scheduled_event(&self, key: &str) -> bool {
        key.starts_with("note_")
    }
}

/// A schedule's writes to `voices`' controls, by voice: `[level, tone,
/// note_on, note_off]`.
fn writes(voices: &[&Arc<Voice>]) -> (Arc<LegacyWrites>, Vec<[usize; 4]>) {
    let mut controls = LegacyControls::default();
    let indices = (voices.iter())
        .map(|voice| {
            let surface: Surface = (*voice).clone();
            KEYS.map(|key| controls.index(&surface, key))
        })
        .collect();
    (controls.build(), indices)
}

fn n(value: f32) -> TableValue {
    TableValue::Number(value)
}

#[test]
fn a_write_finding_its_surface_busy_lands_when_tried_again() {
    let voice = Arc::new(Voice::default());
    let (writes, [level, _, on, _]) = one(&voice);
    voice.busy.store(true, Ordering::Relaxed);
    let (_, allocs, frees) = allocator_events(|| {
        writes.write(on, n(60.0));
        writes.write(level, n(5.0));
        writes.retry();
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(writes.waiting(), 2, "still busy: both wait");
    assert!(voice.landed().is_empty());
    voice.busy.store(false, Ordering::Relaxed);
    let (_, allocs, frees) = allocator_events(|| writes.retry());
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(voice.landed(), [("note_on", 60.0), ("level", 5.0)]);
    assert_eq!((writes.waiting(), writes.refused().count()), (0, 0));
}

#[test]
fn a_later_write_to_a_surface_waits_behind_its_earlier_one() {
    let (voice, other) = (Arc::new(Voice::default()), Arc::new(Voice::default()));
    let (writes, indices) = writes(&[&voice, &other]);
    let ([_, _, on, off], [other_level, ..]) = (indices[0], indices[1]);
    voice.busy.store(true, Ordering::Relaxed);
    writes.write(on, n(60.0));
    voice.busy.store(false, Ordering::Relaxed);
    // Free now, but behind its note_on: a note_off never lands first.
    writes.write(off, n(60.0));
    assert!(voice.landed().is_empty());
    // Another surface is not held back.
    writes.write(other_level, n(3.0));
    assert_eq!(other.landed(), [("level", 3.0)]);
    writes.retry();
    assert_eq!(voice.landed(), [("note_on", 60.0), ("note_off", 60.0)]);
    assert_eq!(writes.waiting(), 0);
}

#[test]
fn a_waiting_value_gives_way_to_a_newer_one_and_an_event_never_does() {
    let voice = Arc::new(Voice::default());
    let (writes, [level, _, on, off]) = one(&voice);
    voice.busy.store(true, Ordering::Relaxed);
    for value in [1.0, 2.0, 3.0] {
        writes.write(level, n(value)); // a ramp, waiting
    }
    writes.write(on, n(60.0));
    writes.write(on, n(60.0)); // a repeated note is two notes
    writes.write(off, n(60.0));
    // Not past the notes between: each level lands where it was written.
    writes.write(level, n(4.0));
    writes.write(level, n(5.0));
    assert_eq!(writes.waiting(), 5);
    voice.busy.store(false, Ordering::Relaxed);
    writes.retry();
    assert_eq!(
        voice.landed(),
        [
            ("level", 3.0),
            ("note_on", 60.0),
            ("note_on", 60.0),
            ("note_off", 60.0),
            ("level", 5.0)
        ]
    );
}

#[test]
fn a_write_past_the_room_or_the_wait_is_dropped_and_counted() {
    let voice = Arc::new(Voice::default());
    let (writes, [_, _, on, _]) = one(&voice);
    voice.busy.store(true, Ordering::Relaxed);
    let (_, allocs, frees) = allocator_events(|| {
        for note in 0..=RETRY_CAPACITY {
            writes.write(on, n(note as f32));
        }
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(writes.waiting(), RETRY_CAPACITY);
    assert_eq!(writes.refused().count(), 1, "no room for the last");
    // 10 ms at 48 kHz in blocks of 128: kept in the first block, then the
    // fourth block's wait is 512.
    for _ in 0..4 {
        writes.age(128, 480);
    }
    assert_eq!(writes.waiting(), RETRY_CAPACITY);
    writes.age(128, 480);
    assert_eq!(writes.waiting(), 0, "waited past the limit");
    assert_eq!(writes.refused().count(), 1 + RETRY_CAPACITY as u32);
}

#[test]
fn a_write_its_surface_refuses_outright_is_counted_never_kept() {
    let voice = Arc::new(Voice::default());
    let (writes, [level, ..]) = one(&voice);
    writes.write(level, TableValue::Bool(true));
    assert_eq!((writes.waiting(), writes.refused().count()), (0, 1));
}

#[test]
fn a_write_kept_late_in_a_long_block_is_tried_again_before_it_gives_up() {
    let voice = Arc::new(Voice::default());
    let (writes, [.., off]) = one(&voice);
    voice.busy.store(true, Ordering::Relaxed);
    writes.write(off, n(60.0));
    // A 1024-frame block ends, longer than the limit: not aged yet.
    writes.age(1024, 480);
    assert_eq!(writes.waiting(), 1);
    voice.busy.store(false, Ordering::Relaxed);
    writes.retry();
    assert_eq!(voice.landed(), [("note_off", 60.0)]);
    // Still busy at its retry, a block later it gives up.
    voice.busy.store(true, Ordering::Relaxed);
    writes.write(off, n(62.0));
    writes.age(1024, 480);
    writes.retry();
    writes.age(1024, 480);
    assert_eq!((writes.waiting(), writes.refused().count()), (0, 1));
}

#[test]
fn a_ramp_starts_from_the_value_waiting_to_land() {
    let voice = Arc::new(Voice::default());
    let (writes, [level, ..]) = one(&voice);
    assert_eq!(writes.read(level), Some(0.0), "the surface's own");
    voice.busy.store(true, Ordering::Relaxed);
    writes.write(level, n(12.0));
    let (read, allocs, frees) = allocator_events(|| writes.read(level));
    assert_eq!((read, allocs, frees), (Some(12.0), 0, 0));
}

/// A development's control fanning out to `voices`' `note_on`.
struct Fan(Vec<Arc<Voice>>);

impl ControlSurface for Fan {
    fn controls(&self) -> Vec<ControlMeta> {
        Vec::new()
    }

    fn get_control(&self, _key: &str) -> Result<ControlValue, String> {
        Ok(ControlValue::Number(0.0))
    }

    fn set_control(&self, _key: &str, _value: ControlValue) -> Result<(), String> {
        Ok(())
    }

    fn scheduled_aliases(&self, key: &str) -> Option<Vec<(Surface, String)>> {
        let voice = |voice: &Arc<Voice>| (voice.clone() as Surface, key.to_string());
        Some(self.0.iter().map(voice).collect())
    }
}

#[test]
fn a_fanned_out_write_waits_only_for_the_alias_that_is_busy() {
    let (free, busy) = (Arc::new(Voice::default()), Arc::new(Voice::default()));
    let fan: Surface = Arc::new(Fan(vec![free.clone(), busy.clone()]));
    let mut controls = LegacyControls::default();
    let on = controls.index(&fan, "note_on");
    let writes = controls.build();
    busy.busy.store(true, Ordering::Relaxed);
    writes.write(on, n(60.0));
    writes.retry();
    assert_eq!(free.landed(), [("note_on", 60.0)], "once, not retriggered");
    assert_eq!(writes.waiting(), 1);
    busy.busy.store(false, Ordering::Relaxed);
    writes.retry();
    assert_eq!(free.landed(), [("note_on", 60.0)]);
    assert_eq!(busy.landed(), [("note_on", 60.0)]);
}

#[test]
fn a_new_schedule_takes_over_the_waits_to_controls_it_still_reaches() {
    let (voice, gone) = (Arc::new(Voice::default()), Arc::new(Voice::default()));
    let (old, indices) = writes(&[&voice, &gone]);
    let ([_, _, on, off], [gone_level, ..]) = (indices[0], indices[1]);
    let (new, [level, ..]) = one(&voice);
    voice.busy.store(true, Ordering::Relaxed);
    gone.busy.store(true, Ordering::Relaxed);
    old.write(on, n(60.0));
    old.write(off, n(60.0));
    old.write(gone_level, n(1.0));
    new.write(level, n(7.0));
    let (_, allocs, frees) = allocator_events(|| new.take_waiting(&old));
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!((old.waiting(), new.waiting()), (0, 3));
    assert_eq!(new.refused().count(), 1, "the write to a surface it lost");
    voice.busy.store(false, Ordering::Relaxed);
    new.retry();
    assert_eq!(
        voice.landed(),
        [("note_on", 60.0), ("note_off", 60.0), ("level", 7.0)],
        "the old schedule's first: no stuck note"
    );
}

#[test]
fn a_write_waits_behind_another_schedule_s_earlier_one_to_its_surface() {
    let voice = Arc::new(Voice::default());
    let ((first, [_, _, on, _]), (second, [level, _, _, off])) = (one(&voice), one(&voice));
    voice.busy.store(true, Ordering::Relaxed);
    first.write(on, n(60.0));
    voice.busy.store(false, Ordering::Relaxed);
    let (_, allocs, frees) = allocator_events(|| {
        // Free now, but behind the first schedule's note_on.
        second.write(off, n(60.0));
        second.write(level, n(3.0));
        second.retry();
    });
    assert_eq!((allocs, frees), (0, 0));
    assert!(voice.landed().is_empty(), "no note_off before its note_on");
    first.retry();
    second.retry();
    assert_eq!(
        voice.landed(),
        [("note_on", 60.0), ("note_off", 60.0), ("level", 3.0)]
    );
    assert_eq!((first.waiting(), second.waiting()), (0, 0));
}

#[test]
fn a_schedule_dropped_holds_no_other_s_writes_back() {
    let voice = Arc::new(Voice::default());
    let ((first, [_, _, on, _]), (second, [.., off])) = (one(&voice), one(&voice));
    voice.busy.store(true, Ordering::Relaxed);
    first.write(on, n(60.0));
    voice.busy.store(false, Ordering::Relaxed);
    drop(first);
    second.write(off, n(60.0));
    assert_eq!(voice.landed(), [("note_off", 60.0)]);
}

/// A schedule's writes to one voice.
fn one(voice: &Arc<Voice>) -> (Arc<LegacyWrites>, [usize; 4]) {
    let (writes, indices) = writes(&[voice]);
    (writes, indices[0])
}
