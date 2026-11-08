//! The payload controls of a declared surface: what a write builds, what
//! reads report for it, and the payloads waiting for a module to build.

use super::options::{Checked, Queued, Write, WriteOptions};
use super::{DeclaredSurface, Written};
use crate::control_request::{ControlIndex, DeclKind, PayloadCodec};
use crate::payload::Payload;
use crate::ControlValue;

#[cfg_attr(not(test), allow(dead_code))]
impl DeclaredSurface {
    /// Records `value` as what payload control `key` reads back as, for
    /// the payload its module was built with, or as another key on the
    /// same inner control wrote it; nothing for a key that names no
    /// control (see [`Self::index`]). A payload still waiting for the
    /// module stays: the bind applies every key's last write in the order
    /// written, so an inner control still takes the later one, and an
    /// alias only the earlier key reaches keeps it. Control thread.
    pub(crate) fn show_payload(&self, key: &str, value: ControlValue) {
        if let Ok(index) = self.index(key) {
            let mut payloads = self.payloads.lock().unwrap();
            match payloads.iter_mut().find(|written| written.index == index) {
                Some(written) => written.shown = value,
                None => payloads.push(Written {
                    index,
                    shown: value,
                    staged: None,
                }),
            }
        }
    }

    /// Writes payload control `key` with a payload built by its module's
    /// own surface rather than a codec (one whose building needs more than
    /// the value written), reading back as `shown`. Control thread.
    pub(crate) fn write_payload(
        &self,
        key: &str,
        payload: Payload,
        shown: ControlValue,
    ) -> Result<(), String> {
        let options = WriteOptions::default();
        self.submit_payload(key, payload, shown, &options).map(drop)
    }

    /// [`Self::write_payload`] as `options` say, saying what the write saw
    /// as it was queued. Control thread.
    pub(crate) fn submit_payload(
        &self,
        key: &str,
        payload: Payload,
        shown: ControlValue,
        options: &WriteOptions,
    ) -> Result<Queued, String> {
        let index = self.payload_index(key)?;
        self.deliver(index, Write::Payload(payload, shown, None), options, None)
    }

    /// [`Self::write_payload`] for a payload built against the live graph's
    /// control-surface directory: refused as [`super::STALE`] when a graph change
    /// published since `checked.generation`, and recorded as
    /// `checked.record` says, submitted as `options` say. Control thread.
    pub(crate) fn write_payload_checked(
        &self,
        (key, payload, shown): (&str, Payload, ControlValue),
        checked: Checked<'_>,
        options: &WriteOptions,
    ) -> Result<Queued, String> {
        let index = self.payload_index(key)?;
        let write = Write::Payload(payload, shown, Some(checked));
        self.deliver(index, write, options, None)
    }

    /// Whether `key` is a payload control.
    pub(crate) fn is_payload(&self, key: &str) -> bool {
        self.payload_decl(key).is_some()
    }

    /// The payload control `key` names, with its codec: `None` when it
    /// names none, or an index past the read-only count it is listed below.
    pub(super) fn payload_decl(&self, key: &str) -> Option<(ControlIndex, Option<PayloadCodec>)> {
        let index = self.index(key).ok()?;
        let (decl, _) = self.table.decl(index)?;
        (decl.kind == DeclKind::Payload).then_some((index, decl.codec))
    }

    /// The payload control `key` names, refused as [`Self::index`] refuses
    /// it, or as no payload control.
    pub(super) fn payload_index(&self, key: &str) -> Result<ControlIndex, String> {
        let index = self.index(key)?;
        match self.table.decl(index) {
            Some((decl, _)) if decl.kind == DeclKind::Payload => Ok(index),
            _ => Err(format!("Control '{key}' is not a payload control")),
        }
    }

    pub(super) fn shown(&self, index: ControlIndex) -> Option<ControlValue> {
        let payloads = self.payloads.lock().unwrap();
        let written = payloads.iter().find(|written| written.index == index)?;
        Some(written.shown.clone())
    }

    /// Records a payload control's write; `staged` replaces any payload
    /// still waiting for the module (dropped here, on a control thread).
    pub(super) fn show(&self, index: ControlIndex, shown: ControlValue, staged: Option<Payload>) {
        let mut payloads = self.payloads.lock().unwrap();
        match payloads.iter_mut().find(|written| written.index == index) {
            Some(written) => {
                written.shown = shown;
                written.staged = staged;
            }
            None => payloads.push(Written {
                index,
                shown,
                staged,
            }),
        }
    }

    /// Builds a payload control's write with its codec, or refuses it.
    pub(super) fn prepare(
        &self,
        key: &str,
        value: &ControlValue,
    ) -> Option<Result<(ControlIndex, Payload, ControlValue), String>> {
        let (index, codec) = self.payload_decl(key)?;
        let Some(codec) = codec else {
            return Some(Err(format!("Control '{key}' cannot be written")));
        };
        let prepared = (codec.prepare)(&self.context(index), value);
        Some(prepared.map(|(payload, shown)| (index, payload, shown)))
    }
}
