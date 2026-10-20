//! A top-level scheduler is played by the graph's request drain, offline
//! and live alike: the curated example renders the same samples both ways.

use std::sync::{Arc, Mutex};

use fugue::{AudioBackend, Invention, InventionBuilder, RenderEngine};

const SAMPLE_RATE: u32 = 48_000;
const BLOCK: usize = 64;
type Render = Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>;

/// A backend that hands its render callback to the test.
struct Capture(Arc<Mutex<Option<Render>>>);

impl AudioBackend for Capture {
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    fn start(&mut self, render: Render) -> Result<(), Box<dyn std::error::Error>> {
        *self.0.lock().unwrap() = Some(render);
        Ok(())
    }

    fn stop(&mut self) {
        self.0.lock().unwrap().take();
    }
}

fn offline(doc: Invention, blocks: usize) -> Vec<f32> {
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_invention(doc).unwrap();
    engine.set_block_size(BLOCK);
    let mut out = vec![0.0f32; blocks * BLOCK * 2];
    engine.render_interleaved(&mut out).unwrap();
    out
}

fn live(doc: Invention, blocks: usize) -> Vec<f32> {
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(doc).unwrap();
    let slot = Arc::new(Mutex::new(None));
    let running = runtime.start_with_backend(Capture(slot.clone())).unwrap();
    let mut out = Vec::with_capacity(blocks * BLOCK * 2);
    {
        let mut slot = slot.lock().unwrap();
        let render = slot.as_mut().unwrap();
        let (mut left, mut right) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
        for _ in 0..blocks {
            render(&mut left, &mut right);
            for (l, r) in left.iter().zip(&right) {
                out.extend([*l, *r]);
            }
        }
    }
    running.stop();
    out
}

#[test]
fn the_curated_example_plays_the_same_samples_offline_and_live() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/control_scheduler.json"
    );
    let doc = Invention::from_file(path).unwrap();
    // 40 beats at 120 bpm, then 90: past the tempo change at step 24 and
    // both hairpins.
    let blocks = SAMPLE_RATE as usize * 22 / BLOCK;
    let rendered = offline(doc.clone(), blocks);
    let heard = live(doc, blocks);
    assert!(rendered.iter().any(|&sample| sample != 0.0));
    let first = rendered.iter().zip(&heard).position(|(a, b)| a != b);
    assert_eq!(first, None, "offline and live differ from this sample on");
}
