use fugue::AudioBackend;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub struct NullAudioBackend {
    sample_rate: u32,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl NullAudioBackend {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            running: Arc::new(AtomicBool::new(false)),
            worker: None,
        }
    }
}

impl AudioBackend for NullAudioBackend {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn start(
        &mut self,
        mut sample_fn: Box<dyn FnMut() -> f32 + Send>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let running = self.running.clone();
        running.store(true, Ordering::SeqCst);

        self.worker = Some(thread::spawn(move || {
            while running.load(Ordering::SeqCst) {
                let _ = sample_fn();
                thread::sleep(Duration::from_millis(1));
            }
        }));

        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);

        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
