//! The audio thread's side of module output taps: the lowest and highest
//! value each of up to two of a module's outputs reached over each run of
//! [`TAP_BIN`] samples, for a live view of what that module plays.
//!
//! Taps live in a fixed set of [`MAX_OUTPUT_TAPS`] slots, each with its own
//! preallocated ring, all built on a control thread as the graph goes live
//! (see [`TapBoard`]). Opening a tap is a store into a free slot that the
//! audio thread takes up at its next segment; it changes nothing anyone
//! hears, so it is not a control request. With no tap open, a segment
//! costs one relaxed load. A full ring drops the bin; nothing allocates,
//! frees, locks or waits.
//!
//! A slot names its module by instance key (see `MirrorModule::instance`):
//! each build of a module gets a fresh one. The graph keeps the key of each
//! module it runs, in module order, swapped in with each publication as it
//! installs (see [`OutputTaps::install`]), so a tap follows its module as
//! edits move it, and finds nothing once it is removed or rebuilt. The
//! control side (`crate::invention::taps`) closes such a tap as the edit is
//! published.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use super::SignalGraph;
use crate::spsc::{Producer, Ring};

/// Taps that can be open at once, across every view.
pub const MAX_OUTPUT_TAPS: usize = 8;

/// Samples each bin summarises: about two thirds of a millisecond at
/// 48 kHz, fine enough for a scope's envelope and far too coarse to carry
/// the audio itself.
pub(crate) const TAP_BIN: usize = 32;

/// Bins each slot's ring holds between two drains: about 0.7 s at 48 kHz,
/// where a view drains every few tens of milliseconds.
pub(crate) const TAP_BIN_CAPACITY: usize = 1024;

/// A port slot left empty: the tap reads one channel.
const NO_PORT: u32 = 0xFFFF;

/// One bin: the lowest and highest value each tapped output reached over
/// [`TAP_BIN`] samples. A NaN sample is skipped; a bin of nothing but NaN
/// reads `min > max`. A full ring drops the bin, which its successor's
/// `start` shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TapBin {
    /// Which opening of its slot it was summarised for, so a reader never
    /// takes bins left over from the tap that held the slot before.
    pub(crate) serial: u32,
    /// Its first sample, on the graph's transport.
    pub(crate) start: u64,
    pub(crate) min: [f32; 2],
    pub(crate) max: [f32; 2],
}

/// The slots, shared by the audio thread and the control side.
pub(crate) struct TapBoard {
    /// Slots open: with none, the audio thread looks no further.
    open: AtomicUsize,
    /// Cleared as the graph fails or goes (see [`OutputTaps::stop`]): its
    /// taps will read nothing more.
    running: AtomicBool,
    pub(crate) slots: [TapSlot; MAX_OUTPUT_TAPS],
}

/// One slot: what it taps, and the ring its bins go through.
pub(crate) struct TapSlot {
    /// `serial << 32 | instance key` while open; 0 while closed. Stored
    /// with `Release` after `ports`.
    state: AtomicU64,
    /// The tapped output ports, by index: the first in the low 16 bits,
    /// the second (or [`NO_PORT`]) in the high.
    ports: AtomicU32,
    pub(crate) ring: Arc<Ring<TapBin>>,
}

impl TapBoard {
    /// Control thread: the slots and their rings. Allocates.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            open: AtomicUsize::new(0),
            running: AtomicBool::new(true),
            slots: std::array::from_fn(|_| TapSlot {
                state: AtomicU64::new(0),
                ports: AtomicU32::new(0),
                ring: Ring::with_capacity(TAP_BIN_CAPACITY),
            }),
        })
    }

    /// Control thread: opens closed slot `slot` on `ports` (one or two) of
    /// the module instance `key`, for the opening numbered `serial`
    /// (nonzero). Its bins start at the audio thread's next segment.
    pub(crate) fn open(&self, slot: usize, serial: u32, key: u32, ports: [usize; 2]) {
        let slot = &self.slots[slot];
        let packed = |port: usize| u32::try_from(port).map_or(NO_PORT, |port| port.min(NO_PORT));
        slot.ports
            .store(packed(ports[0]) | packed(ports[1]) << 16, Ordering::Relaxed);
        // Release: a segment that takes up this state sees its ports.
        slot.state
            .store(u64::from(serial) << 32 | u64::from(key), Ordering::Release);
        self.open.fetch_add(1, Ordering::Release);
    }

    /// Whether the graph tapping these slots still runs.
    pub(crate) fn running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Control thread: closes open slot `slot`. A segment already reading
    /// it may still send a bin or two under the old serial.
    pub(crate) fn close(&self, slot: usize) {
        self.slots[slot].state.store(0, Ordering::Release);
        self.open.fetch_sub(1, Ordering::Release);
    }
}

/// The graph's end of the taps: a writer per slot, and the instance key of
/// each module it runs.
pub(crate) struct OutputTaps {
    board: Arc<TapBoard>,
    writers: [TapWriter; MAX_OUTPUT_TAPS],
    /// Each running module's instance key, in module order.
    keys: Vec<u32>,
    /// A publication installed since the writers last found their modules.
    moved: bool,
}

/// One slot's producer, and the bin it is filling.
struct TapWriter {
    producer: Producer<TapBin>,
    /// The slot state this writer took up; 0 for none.
    state: u64,
    module_idx: Option<usize>,
    ports: [Option<usize>; 2],
    bin: TapBin,
    /// Samples in `bin` so far.
    filled: usize,
}

impl OutputTaps {
    /// Control thread: claims the producer end of each of `board`'s rings,
    /// for a graph running modules with instance keys `keys`.
    pub(crate) fn new(board: Arc<TapBoard>, keys: Vec<u32>) -> Self {
        let writers = std::array::from_fn(|slot| TapWriter {
            producer: Producer::claim(Arc::clone(&board.slots[slot].ring)),
            state: 0,
            module_idx: None,
            ports: [None; 2],
            bin: TapBin {
                serial: 0,
                start: 0,
                min: [0.0; 2],
                max: [0.0; 2],
            },
            filled: 0,
        });
        Self {
            board,
            writers,
            keys,
            moved: true,
        }
    }

    /// Audio thread, as a publication installs: swaps in its instance keys,
    /// leaving the ones it replaces in `keys` to be freed with it. Each tap
    /// finds its module again at its next segment. Allocation-free.
    pub(crate) fn install(&mut self, keys: &mut Vec<u32>) {
        std::mem::swap(&mut self.keys, keys);
        self.moved = true;
    }

    /// Closes every tap on this graph, for good: it failed, or is going.
    /// Allocation- and lock-free.
    pub(crate) fn stop(&self) {
        self.board.running.store(false, Ordering::Release);
    }
}

impl Drop for OutputTaps {
    /// The invention stopped, or its graph was replaced.
    fn drop(&mut self) {
        self.stop();
    }
}

impl TapWriter {
    /// Takes up slot state `state`, finding its module among `keys`.
    fn take_up(&mut self, slot: &TapSlot, state: u64, keys: &[u32]) {
        let serial = (state >> 32) as u32;
        if serial != self.bin.serial {
            self.filled = 0;
        }
        let key = state as u32;
        let ports = slot.ports.load(Ordering::Relaxed);
        let port = |bits: u32| (bits != NO_PORT).then_some(bits as usize);
        self.ports = [port(ports & 0xFFFF), port(ports >> 16)];
        self.module_idx = keys.iter().position(|&k| k == key);
        self.bin.serial = serial;
        // A close and a reopen while the ports were read: take it up again
        // next segment.
        self.state = if slot.state.load(Ordering::Acquire) == state {
            state
        } else {
            0
        };
    }

    /// Folds `frames` samples of the module's `outputs` (`capacity` samples
    /// a port), the first at sample `start`, into bins, sending each one
    /// filled.
    fn fold(&mut self, outputs: &[f32], capacity: usize, frames: usize, start: u64) {
        let channel = |port: Option<usize>| {
            port.and_then(|port| outputs.get(port * capacity..port * capacity + frames))
        };
        let (Some(first), second) = (channel(self.ports[0]), channel(self.ports[1])) else {
            self.filled = 0;
            return;
        };
        let mut i = 0;
        while i < frames {
            if self.filled == 0 {
                self.bin.start = start + i as u64;
                self.bin.min = [f32::INFINITY; 2];
                self.bin.max = [f32::NEG_INFINITY; 2];
            }
            let run = (TAP_BIN - self.filled).min(frames - i);
            extend(&mut self.bin, 0, &first[i..i + run]);
            if let Some(second) = second {
                extend(&mut self.bin, 1, &second[i..i + run]);
            }
            self.filled += run;
            i += run;
            if self.filled == TAP_BIN {
                self.filled = 0;
                // A full ring drops it: the reader sees the gap.
                let _ = self.producer.push(self.bin);
            }
        }
    }
}

/// Widens channel `channel` of `bin` to cover `samples`.
#[inline]
fn extend(bin: &mut TapBin, channel: usize, samples: &[f32]) {
    let (min, max) = samples
        .iter()
        .fold((bin.min[channel], bin.max[channel]), |(min, max), &s| {
            (min.min(s), max.max(s))
        });
    bin.min[channel] = min;
    bin.max[channel] = max;
}

impl SignalGraph {
    /// Audio thread, after each segment of `frames` samples: folds each
    /// open tap's outputs into its bins. With no tap open, one relaxed
    /// load. Reads only the output buffers; allocation-, free- and
    /// lock-free.
    pub(super) fn tap_outputs(&mut self, frames: usize) {
        let Self {
            output_taps: Some(taps),
            out_bufs,
            block_capacity,
            current_sample,
            ..
        } = self
        else {
            return;
        };
        if taps.board.open.load(Ordering::Relaxed) == 0 || frames == 0 {
            return;
        }
        let start = *current_sample - frames as u64;
        let board = &*taps.board;
        for (slot, writer) in board.slots.iter().zip(taps.writers.iter_mut()) {
            let state = slot.state.load(Ordering::Acquire);
            if state == 0 {
                writer.state = 0;
                continue;
            }
            if state != writer.state || taps.moved {
                writer.take_up(slot, state, &taps.keys);
            }
            let outputs = writer.module_idx.and_then(|idx| out_bufs.get(idx));
            match outputs {
                Some(outputs) if writer.state != 0 => {
                    writer.fold(outputs, *block_capacity, frames, start);
                }
                _ => writer.filled = 0,
            }
        }
        taps.moved = false;
    }
}

#[cfg(test)]
mod tests;
