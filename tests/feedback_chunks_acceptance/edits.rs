//! Criterion "live edits recompute chunks": an edit that breaks the chunk
//! condition (an in-loop module patched into the delay's `time_mod`) switches
//! the loop to per-sample (time + 1); an edit that restores it (disconnecting)
//! switches it back (exact). Offline edits, live edits and live reloads.

use super::*;

/// 576 blocks of 64 frames, then two phases of 750 (one second each). A gate
/// (bpm 60) sits mid-way through each of the last two phases and at the start
/// of the first; the repeats of each fit well inside their phase.
const PHASES: [usize; 3] = [576, 750, 750];
const D: usize = 3000;

trait Rig {
    fn blocks(&mut self, blocks: usize) -> (Vec<f32>, Vec<f32>);
    fn patch(&mut self);
    fn unpatch(&mut self);
}

fn base(min_time: Option<f64>) -> Doc {
    with_mod_tap(gate_loop(delay_cfg(0.0625, min_time), 0.7, 60.0))
}

struct Offline(RenderEngine);
impl Rig for Offline {
    fn blocks(&mut self, blocks: usize) -> (Vec<f32>, Vec<f32>) {
        render(&mut self.0, blocks * BLOCK)
    }
    fn patch(&mut self) {
        self.0
            .connect("m", "audio", "d", "time_mod")
            .expect("connects");
    }
    fn unpatch(&mut self) {
        self.0
            .disconnect("m", "audio", "d", "time_mod")
            .expect("disconnects");
    }
}

struct LiveEdits(Live);
impl Rig for LiveEdits {
    fn blocks(&mut self, blocks: usize) -> (Vec<f32>, Vec<f32>) {
        self.0.render(blocks)
    }
    fn patch(&mut self) {
        self.0
            .running
            .connect("m", "audio", "d", "time_mod")
            .expect("connects");
    }
    fn unpatch(&mut self) {
        self.0
            .running
            .disconnect("m", "audio", "d", "time_mod")
            .expect("disconnects");
    }
}

struct LiveReload(Live, Doc);
impl Rig for LiveReload {
    fn blocks(&mut self, blocks: usize) -> (Vec<f32>, Vec<f32>) {
        self.0.render(blocks)
    }
    fn patch(&mut self) {
        let doc = patch_mod(self.1.clone());
        self.0.reload(&doc);
    }
    fn unpatch(&mut self) {
        let doc = self.1.clone();
        self.0.reload(&doc);
    }
}

fn script(rig: &mut impl Rig, label: &str) {
    let phase = |(left, right): (Vec<f32>, Vec<f32>)| echo_offsets(&left, &right, 0, left.len());
    let first = phase(rig.blocks(PHASES[0]));
    rig.patch();
    let second = phase(rig.blocks(PHASES[1]));
    rig.unpatch();
    let third = phase(rig.blocks(PHASES[2]));
    assert_eq!(first[..6], exact(D, 6)[..], "{label}: before the edit");
    assert_eq!(
        second[..6],
        per_sample(D, 6)[..],
        "{label}: with the in-loop module on time_mod"
    );
    assert_eq!(
        third[..6],
        exact(D, 6)[..],
        "{label}: after disconnecting it"
    );
}

#[test]
fn offline_edits_recompute_chunks() {
    for min_time in [None, Some(0.01)] {
        script(
            &mut Offline(engine(&base(min_time))),
            &format!("offline, min_time {min_time:?}"),
        );
    }
}

#[test]
fn live_connect_and_disconnect_recompute_chunks() {
    for min_time in [None, Some(0.01)] {
        let mut rig = LiveEdits(Live::start(&base(min_time)));
        script(&mut rig, &format!("live edits, min_time {min_time:?}"));
        rig.0.stop();
    }
}

#[test]
fn live_reload_recomputes_chunks() {
    let doc = base(Some(0.01));
    let mut rig = LiveReload(Live::start(&doc), doc);
    script(&mut rig, "live reload");
    rig.0.stop();
}
