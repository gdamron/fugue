use fugue::AudioBackend;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
        mut render: Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let running = self.running.clone();
        running.store(true, Ordering::SeqCst);

        self.worker = Some(thread::spawn(move || {
            let mut left = [0.0f32; 64];
            let mut right = [0.0f32; 64];
            while running.load(Ordering::SeqCst) {
                render(&mut left, &mut right);
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
