//! The audio ring the audio thread pushes into and the video frame queue.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub(super) struct AudioFrameRing {
    slots: Box<[AtomicU64]>,
    read_index: AtomicUsize,
    write_index: AtomicUsize,
    capacity: usize,
    pub(super) dropped: AtomicUsize,
}

impl AudioFrameRing {
    pub(super) fn new(capacity: usize) -> Self {
        let slots = (0..capacity)
            .map(|_| AtomicU64::new(0))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            slots,
            read_index: AtomicUsize::new(0),
            write_index: AtomicUsize::new(0),
            capacity,
            dropped: AtomicUsize::new(0),
        }
    }

    #[inline]
    pub(super) fn push(&self, left: f32, right: f32) {
        let write = self.write_index.load(Ordering::Relaxed);
        let next = self.advance(write);
        if next == self.read_index.load(Ordering::Acquire) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }

        self.slots[write].store(pack_frame(left, right), Ordering::Relaxed);
        self.write_index.store(next, Ordering::Release);
    }

    pub(super) fn pop(&self) -> Option<(f32, f32)> {
        let read = self.read_index.load(Ordering::Relaxed);
        if read == self.write_index.load(Ordering::Acquire) {
            return None;
        }

        let frame = unpack_frame(self.slots[read].load(Ordering::Relaxed));
        self.read_index.store(self.advance(read), Ordering::Release);
        Some(frame)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.read_index.load(Ordering::Acquire) == self.write_index.load(Ordering::Acquire)
    }

    #[inline]
    fn advance(&self, index: usize) -> usize {
        let next = index + 1;
        if next == self.capacity {
            0
        } else {
            next
        }
    }
}

pub(super) struct VideoFrameQueue {
    frames: Vec<Option<Box<[u8]>>>,
    capacity: usize,
    frame_bytes: usize,
    read_index: usize,
    write_index: usize,
    len: usize,
    pub(super) dropped: usize,
}

impl VideoFrameQueue {
    pub(super) fn new(capacity: usize, frame_bytes: usize) -> Self {
        Self {
            frames: (0..capacity).map(|_| None).collect(),
            capacity,
            frame_bytes,
            read_index: 0,
            write_index: 0,
            len: 0,
            dropped: 0,
        }
    }

    pub(super) fn push(&mut self, frame: &[u8]) -> Result<(), String> {
        if frame.len() != self.frame_bytes {
            return Err(format!(
                "rtmp_sink expected RGBA frame with {} bytes, got {}",
                self.frame_bytes,
                frame.len()
            ));
        }
        if self.len == self.capacity {
            self.read_index = self.advance(self.read_index);
            self.len -= 1;
            self.dropped += 1;
        }
        match &mut self.frames[self.write_index] {
            Some(slot) => slot.copy_from_slice(frame),
            slot @ None => *slot = Some(frame.to_vec().into_boxed_slice()),
        }
        self.write_index = self.advance(self.write_index);
        self.len += 1;
        Ok(())
    }

    pub(super) fn pop_into(&mut self, frame: &mut [u8]) -> bool {
        if self.len == 0 {
            return false;
        }
        frame.copy_from_slice(
            self.frames[self.read_index]
                .as_deref()
                .expect("queued video frame slot is initialized"),
        );
        self.read_index = self.advance(self.read_index);
        self.len -= 1;
        true
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    fn advance(&self, index: usize) -> usize {
        let next = index + 1;
        if next == self.capacity {
            0
        } else {
            next
        }
    }
}

#[inline]
fn pack_frame(left: f32, right: f32) -> u64 {
    ((left.to_bits() as u64) << 32) | right.to_bits() as u64
}

#[inline]
fn unpack_frame(frame: u64) -> (f32, f32) {
    (
        f32::from_bits((frame >> 32) as u32),
        f32::from_bits(frame as u32),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_queue_reuses_allocated_slots_and_drops_oldest_frame() {
        let mut queue = VideoFrameQueue::new(2, 4);
        queue.push(&[1; 4]).unwrap();
        queue.push(&[2; 4]).unwrap();
        let slot_addresses: Vec<usize> = queue
            .frames
            .iter()
            .map(|frame| frame.as_deref().unwrap().as_ptr() as usize)
            .collect();
        queue.push(&[3; 4]).unwrap();

        let mut frame = [0; 4];
        assert!(queue.pop_into(&mut frame));
        assert_eq!(frame, [2; 4]);
        assert!(queue.pop_into(&mut frame));
        assert_eq!(frame, [3; 4]);
        assert!(!queue.pop_into(&mut frame));
        assert_eq!(queue.dropped, 1);
        assert_eq!(
            queue
                .frames
                .iter()
                .map(|frame| frame.as_deref().unwrap().as_ptr() as usize)
                .collect::<Vec<_>>(),
            slot_addresses
        );
    }
}
