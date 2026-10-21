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
fn an_event_waits_until_it_lands_or_finds_no_room() {
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
    // 10 ms at 48 kHz in blocks of 128, many times over: a note waits on.
    for _ in 0..20 {
        writes.age(128, 480);
    }
    assert_eq!(writes.waiting(), RETRY_CAPACITY);
    voice.busy.store(false, Ordering::Relaxed);
    writes.retry();
    assert_eq!(voice.landed().len(), RETRY_CAPACITY);
}

#[test]
fn a_value_waiting_past_the_limit_is_dropped_and_counted() {
    let voice = Arc::new(Voice::default());
    let (writes, [level, _, _, off]) = one(&voice);
    voice.busy.store(true, Ordering::Relaxed);
    writes.write(level, n(1.0));
    writes.write(off, n(60.0));
    // Kept in the first block, then the fourth block's wait is 512.
    for _ in 0..4 {
        writes.age(128, 480);
    }
    assert_eq!(writes.waiting(), 2);
    writes.age(128, 480);
    assert_eq!((writes.waiting(), writes.refused().count()), (1, 1));
    voice.busy.store(false, Ordering::Relaxed);
    writes.retry();
    assert_eq!(voice.landed(), [("note_off", 60.0)]);
}

#[test]
fn values_crowding_the_room_never_crowd_out_the_note_off_after_them() {
    let voice = Arc::new(Voice::default());
    let (writes, [level, tone, _, off]) = one(&voice);
    voice.busy.store(true, Ordering::Relaxed);
    let (_, allocs, frees) = allocator_events(|| {
        // Two ramps at once: neither's writes are ever the surface's last.
        for value in 0..100 {
            writes.write(level, n(value as f32));
            writes.write(tone, n(value as f32));
        }
        writes.write(off, n(60.0));
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(
        writes.refused().count(),
        0,
        "each ramp's newest took its place"
    );
    voice.busy.store(false, Ordering::Relaxed);
    writes.retry();
    let landed = voice.landed();
    assert_eq!(landed.len(), RETRY_CAPACITY - EVENT_ROOM + 1);
    assert_eq!(
        landed[landed.len() - 3..],
        [("level", 99.0), ("tone", 99.0), ("note_off", 60.0)],
        "each ramp's last value, then the note off"
    );
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
    let (writes, [level, ..]) = one(&voice);
    voice.busy.store(true, Ordering::Relaxed);
    writes.write(level, n(1.0));
    // A 1024-frame block ends, longer than the limit: not aged yet.
    writes.age(1024, 480);
    assert_eq!(writes.waiting(), 1);
    voice.busy.store(false, Ordering::Relaxed);
    writes.retry();
    assert_eq!(voice.landed(), [("level", 1.0)]);
    // Still busy at its retry, a block later it gives up.
    voice.busy.store(true, Ordering::Relaxed);
    writes.write(level, n(2.0));
    writes.age(1024, 480);
    writes.retry();
    writes.age(1024, 480);
    assert_eq!((writes.waiting(), writes.refused().count()), (0, 1));
}

#[test]
fn a_ramp_starts_from_the_value_waiting_to_land() {
    let voice = Arc::new(Voice::default());
    let (writes, [level, ..]) = one(&voice);
    assert_eq!(writes.read(level, &[]), Some(0.0), "the surface's own");
    voice.busy.store(true, Ordering::Relaxed);
    writes.write(level, n(12.0));
    let (read, allocs, frees) = allocator_events(|| writes.read(level, &[]));
    assert_eq!((read, allocs, frees), (Some(12.0), 0, 0));
}

/// A development's control writing through to `aliases`, whatever its key.
struct Fan(Vec<(Arc<Voice>, &'static str)>);

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

    fn scheduled_aliases(&self, _key: &str) -> Option<Vec<(Surface, String)>> {
        let alias = |(voice, key): &(Arc<Voice>, &str)| (voice.clone() as Surface, key.to_string());
        Some(self.0.iter().map(alias).collect())
    }
}

/// A schedule's write to a development's control fanning out to `aliases`,
/// and to `voice`'s note_off.
fn fanned(aliases: Vec<(Arc<Voice>, &'static str)>) -> (Arc<LegacyWrites>, usize, usize) {
    let voice: Surface = aliases[0].0.clone();
    let fan: Surface = Arc::new(Fan(aliases));
    let mut controls = LegacyControls::default();
    let fanned = controls.index(&fan, "any");
    let off = controls.index(&voice, "note_off");
    (controls.build(), fanned, off)
}

#[test]
fn a_fanned_out_write_waits_only_for_the_alias_that_is_busy() {
    let (free, busy) = (Arc::new(Voice::default()), Arc::new(Voice::default()));
    let (writes, on, _) = fanned(vec![(free.clone(), "note_on"), (busy.clone(), "note_on")]);
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
fn a_ramp_fanning_out_to_two_controls_of_one_module_leaves_room_for_its_note_off() {
    let voice = Arc::new(Voice::default());
    let (writes, fanned, off) = fanned(vec![(voice.clone(), "level"), (voice.clone(), "tone")]);
    voice.busy.store(true, Ordering::Relaxed);
    for value in 0..100 {
        writes.write(fanned, n(value as f32));
    }
    writes.write(off, n(60.0));
    voice.busy.store(false, Ordering::Relaxed);
    writes.retry();
    let landed = voice.landed();
    assert_eq!(
        landed[landed.len() - 3..],
        [("level", 99.0), ("tone", 99.0), ("note_off", 60.0)]
    );
    assert_eq!(writes.refused().count(), 0);
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

#[test]
fn a_value_crowded_out_of_room_still_lands_after_another_schedule_s_older_one() {
    let voice = Arc::new(Voice::default());
    let ((first, [level, tone, ..]), (second, [other, ..])) = (one(&voice), one(&voice));
    voice.busy.store(true, Ordering::Relaxed);
    // Two ramps at once fill the first schedule's room for values.
    for value in 0..(RETRY_CAPACITY - EVENT_ROOM) / 2 {
        first.write(level, n(value as f32));
        first.write(tone, n(value as f32));
    }
    second.write(other, n(9.0));
    let (_, allocs, frees) = allocator_events(|| first.write(level, n(12.0)));
    assert_eq!((allocs, frees), (0, 0));
    voice.busy.store(false, Ordering::Relaxed);
    for _ in 0..2 {
        first.retry();
        second.retry();
    }
    let levels: Vec<f32> = (voice.landed().into_iter())
        .filter_map(|(key, value)| (key == "level").then_some(value))
        .collect();
    assert_eq!(levels[levels.len() - 2..], [9.0, 12.0], "the newest last");
    assert_eq!((first.waiting(), second.waiting()), (0, 0));
    assert_eq!(first.refused().count() + second.refused().count(), 0);
}

#[test]
fn a_left_event_lets_its_surface_go_once_nothing_else_holds_it() {
    let voice = Arc::new(Voice::default());
    let (writes, [.., off]) = one(&voice);
    voice.busy.store(true, Ordering::Relaxed);
    writes.write(off, n(60.0));
    writes.abandon();
    drop(writes);
    // Its module gone from the graph, nothing will make room on it.
    let gone = Arc::downgrade(&voice);
    drop(voice);
    super::land_left_events();
    assert!(gone.upgrade().is_none(), "not held for good");
}

/// A schedule's writes to one voice.
fn one(voice: &Arc<Voice>) -> (Arc<LegacyWrites>, [usize; 4]) {
    let (writes, indices) = writes(&[voice]);
    (writes, indices[0])
}
