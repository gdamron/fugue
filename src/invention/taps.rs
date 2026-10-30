//! Taps on a module's outputs, for live views: a scope following what one
//! module plays.
//!
//! A tap names a module and one or two of its outputs. The audio thread
//! summarises them into the lowest and highest value over each run of
//! samples (see `graph::taps`), and only those bins leave it: never the
//! audio itself. At most [`MAX_OUTPUT_TAPS`] are open at once. A tap stays
//! open until its [`OutputTap`] is dropped, or until an edit removes or
//! rebuilds its module (a swap, or a reload that rebuilds it): the tap
//! closes as that edit is published, and a view opens a new one on the
//! module now under that id if it wants it.

use std::sync::Arc;

pub use super::graph::taps::MAX_OUTPUT_TAPS;
use super::graph::taps::{TapBin, TapBoard};
use super::publish::TopologyMirror;
use super::runtime::GraphCommandError;
use crate::audio_thread::ControlLock;

/// The control side of the output taps: which slot each open tap holds.
pub(crate) struct TapRegistry {
    board: Arc<TapBoard>,
    slots: ControlLock<Slots>,
}

#[derive(Default)]
struct Slots {
    open: [Option<Opened>; MAX_OUTPUT_TAPS],
    /// The serial the latest opening took.
    serial: u32,
}

/// An open slot: which opening, of which module instance.
#[derive(Clone, Copy)]
struct Opened {
    serial: u32,
    instance: u32,
}

impl TapRegistry {
    /// The registry and the slots it hands out. Control thread: allocates.
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            board: TapBoard::new(),
            slots: ControlLock::new(Slots::default()),
        })
    }

    pub(crate) fn board(&self) -> &Arc<TapBoard> {
        &self.board
    }

    /// Opens a tap on `ports` of `module_id` as `mirror` (the topology as
    /// last published) runs it: the named ones (one or two), or with none
    /// named, the module's audio (see [`default_ports`]). Its bins start
    /// once the module is running. Call under the publisher, so no edit
    /// publishes meanwhile.
    pub(crate) fn open(
        self: &Arc<Self>,
        mirror: &TopologyMirror,
        module_id: &str,
        ports: &[&str],
    ) -> Result<OutputTap, GraphCommandError> {
        let module = (mirror.modules.get(module_id))
            .ok_or_else(|| GraphCommandError::UnknownModule(module_id.to_string()))?;
        let outputs = &module.ports.outputs;
        let names: Vec<String> = if ports.is_empty() {
            default_ports(outputs)
        } else {
            ports.iter().map(|port| port.to_string()).collect()
        };
        if names.is_empty() || names.len() > 2 {
            let message = match names.len() {
                0 => format!("module '{module_id}' has no outputs to tap"),
                n => format!("a tap reads one or two outputs, not {n}"),
            };
            return Err(GraphCommandError::InvalidPort(message));
        }
        let index = |name: &String| {
            outputs.iter().position(|port| port == name).ok_or_else(|| {
                GraphCommandError::InvalidPort(format!(
                    "module '{module_id}' does not have output port '{name}' (available: {outputs:?})"
                ))
            })
        };
        let first = index(&names[0])?;
        let second = names.get(1).map(index).transpose()?.unwrap_or(usize::MAX);

        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        let Some(slot) = slots.open.iter().position(Option::is_none) else {
            return Err(GraphCommandError::ControlError(format!(
                "{MAX_OUTPUT_TAPS} output taps are open, the most at once; close one first"
            )));
        };
        slots.serial = slots.serial.wrapping_add(1).max(1);
        let opened = Opened {
            serial: slots.serial,
            instance: module.instance,
        };
        slots.open[slot] = Some(opened);
        // Bins the slot's last tap left unread; one it sends as it closes
        // still may come, under its own serial.
        self.board.slots[slot].ring.drain();
        (self.board).open(slot, opened.serial, opened.instance, [first, second]);
        Ok(OutputTap {
            registry: Arc::clone(self),
            slot,
            serial: opened.serial,
            module_id: module_id.to_string(),
            ports: names,
            sample_rate: 0,
            pending: Vec::new(),
            next: None,
        })
    }

    /// As `mirror` is published, closes every tap on a module instance it
    /// no longer runs: one removed, or rebuilt under the same id. Call
    /// under the publisher.
    pub(crate) fn close_gone(&self, mirror: &TopologyMirror) {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        for slot in 0..MAX_OUTPUT_TAPS {
            let Some(opened) = slots.open[slot] else {
                continue;
            };
            let running =
                (mirror.modules.values()).any(|module| module.instance == opened.instance);
            if !running {
                slots.open[slot] = None;
                self.board.close(slot);
            }
        }
    }

    /// Holds the registry's lock, as a control thread opening, reading or
    /// closing a tap does.
    #[cfg(test)]
    pub(crate) fn hold(&self) -> impl Drop + '_ {
        self.slots.lock().unwrap()
    }

    /// Closes `slot` if opening `serial` still holds it.
    fn close(&self, slot: usize, serial: u32) {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if slots.open[slot].is_some_and(|opened| opened.serial == serial) {
            slots.open[slot] = None;
            self.board.close(slot);
        }
    }
}

/// The outputs a tap reads when none are named: `audio_left` and
/// `audio_right` when the module has both, else `audio`, else its first
/// output.
pub(crate) fn default_ports(outputs: &[String]) -> Vec<String> {
    let has = |name: &str| outputs.iter().any(|port| port == name);
    let names: &[&str] = if has("audio_left") && has("audio_right") {
        &["audio_left", "audio_right"]
    } else if has("audio") {
        &["audio"]
    } else {
        return outputs.iter().take(1).cloned().collect();
    };
    names.iter().map(|name| name.to_string()).collect()
}

/// An open tap on a module's outputs. Dropping it closes the tap.
pub struct OutputTap {
    registry: Arc<TapRegistry>,
    slot: usize,
    serial: u32,
    module_id: String,
    ports: Vec<String>,
    /// Samples a second, for what it reads.
    pub(crate) sample_rate: u32,
    /// Bins taken from the ring and not yet read (see `summary`).
    pending: Vec<TapBin>,
    /// The sample after the last one read; `None` before the first read.
    next: Option<u64>,
}

impl OutputTap {
    /// The module it taps.
    pub fn module_id(&self) -> &str {
        &self.module_id
    }

    /// The outputs it reads, one per channel.
    pub fn ports(&self) -> &[String] {
        &self.ports
    }

    /// Moves every bin the audio thread sent since the last call onto the
    /// pending ones, oldest last, and returns whether the tap is still
    /// open: false once an edit removed or rebuilt its module, or the
    /// invention stopped. Control
    /// thread; never blocks the audio thread.
    fn take_bins(&mut self) -> bool {
        let registry = &*self.registry;
        let slots = registry.slots.lock().unwrap_or_else(|e| e.into_inner());
        let open = slots.open[self.slot].is_some_and(|opened| opened.serial == self.serial);
        if !open || !registry.board.running() {
            // The slot may be another tap's by now: its bins are not ours.
            return false;
        }
        let ring = &registry.board.slots[self.slot].ring;
        while let Some(bin) = ring.pop() {
            if bin.serial == self.serial {
                self.pending.push(bin);
            }
        }
        true
    }
}

impl Drop for OutputTap {
    fn drop(&mut self) {
        self.registry.close(self.slot, self.serial);
    }
}

mod summary;
#[cfg(test)]
mod tests;

pub use summary::{ChannelBins, OutputBins, MAX_TAP_BINS};
