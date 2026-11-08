//! A declared surface as a module's [`ControlSurface`].

use super::options::{Queued, WriteOptions};
use super::{Declaration, DeclaredSurface, Route, Staged};
use crate::control_request::{apply_declared, Automation, DeclKind, Writer};
use crate::payload::Payload;
use crate::traits::ControlSurfaceMap;
use crate::{ControlMeta, ControlSurface, ControlValue, Module};

impl ControlSurface for DeclaredSurface {
    fn controls(&self) -> Vec<ControlMeta> {
        self.table.metas(|index| {
            let value = self.cells.load(index)?;
            self.table.value(index, value).or_else(|| self.shown(index))
        })
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        if let Some((index, _)) = self.payload_decl(key) {
            return self
                .shown(index)
                .ok_or_else(|| format!("Control '{key}' holds no value yet"));
        }
        let index = self.index(key)?;
        self.cells
            .load(index)
            .and_then(|value| self.table.value(index, value))
            .ok_or_else(|| format!("Unknown control: {key}"))
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        self.submit(key, value, &WriteOptions::default()).map(drop)
    }

    #[allow(private_interfaces)]
    fn submit(
        &self,
        key: &str,
        value: ControlValue,
        options: &WriteOptions,
    ) -> Result<Queued, String> {
        self.set_control_queued(key, value, options)
    }

    fn validate_control(
        &self,
        key: &str,
        value: &ControlValue,
        _surfaces: &ControlSurfaceMap,
    ) -> Result<(), String> {
        let decl = (self.index(key).ok()).and_then(|index| self.table.decl(index));
        if decl.is_some_and(|(decl, _)| decl.loads_on_write) {
            return value.as_string().map(drop);
        }
        if let Some(prepared) = self.prepare(key, value) {
            return prepared.map(drop);
        }
        let index = self.index(key)?;
        self.coerce(index, value).map(drop)
    }

    /// A payload built with this surface's context, which a development
    /// aliasing it does not hold: `None` when it holds none, and the
    /// development's own surface builds the write from the value alone.
    #[allow(private_interfaces)]
    fn prepare_payload(
        &self,
        key: &str,
        value: &ControlValue,
    ) -> Option<Result<(Payload, ControlValue), String>> {
        if self.context.is_empty() {
            return None;
        }
        let prepared = self.prepare(key, value)?;
        Some(prepared.map(|(_, payload, shown)| (payload, shown)))
    }

    fn bind(&self, route: Route, module: &mut dyn Module) {
        let mut current = self.route.lock().unwrap();
        // A retired surface's module was displaced; it never runs again.
        if matches!(*current, Route::Retired) {
            return;
        }
        debug_assert!(
            module
                .declared()
                .is_some_and(|(_, cells)| std::ptr::eq(cells, &*self.cells)),
            "a declared surface binds only its own module"
        );
        if let Route::Building(written) = &*current {
            // In the order written, each scalar as written: read before any
            // payload applies, as a payload republishes the cells it
            // rewrites. What a payload replaces, or a refused one, is
            // dropped here, on this control thread.
            let values: Vec<_> = (written.iter())
                .map(|staged| match *staged {
                    Staged::Control(index) => (index, self.cells.load(index), true),
                    Staged::Event(index, value) => (index, Some(value), false),
                })
                .collect();
            let mut payloads = self.payloads.lock().unwrap();
            for (index, value, control) in values {
                let payload =
                    (payloads.iter_mut()).find(|payload| control && payload.index == index);
                match payload {
                    Some(payload) => {
                        if let Some(staged) = payload.staged.take() {
                            let applied = module.apply_payload(index, staged);
                            debug_assert!(applied.is_ok(), "a module takes what its codec builds");
                        }
                    }
                    None => {
                        if let Some(value) = value {
                            let _ = apply_declared(module, index, value);
                        }
                    }
                }
            }
        }
        *current = route;
    }

    fn activate(&self, route: Route) {
        let mut current = self.route.lock().unwrap();
        if matches!(*current, Route::Prepared) {
            *current = route;
        }
    }

    fn retire(&self) {
        *self.route.lock().unwrap() = Route::Retired;
    }

    fn declares(&self, key: &str) -> bool {
        self.index(key).is_ok()
    }

    fn set_undeclared(&self, key: &str, _value: ControlValue) -> Result<(), String> {
        self.index(key).map(drop)
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn declaration(&self, key: &str) -> Option<Declaration> {
        let index = self.index(key).ok()?;
        Some(Declaration {
            index,
            decl: self.table.decl(index)?.0.clone(),
            current: self.cells.load(index)?,
            queued: self.cells.queues_events(),
        })
    }

    fn automation(&self, key: &str) -> Option<Automation> {
        let index = self.index(key).ok()?;
        let (decl, _) = self.table.decl(index)?;
        // An event is automated too: a schedule entry on it fires it, at
        // most once a block (automation writes coalesce per block).
        // A control its module reads (a kit's `play`, naming a slot) is not:
        // a schedule writes the raw value, not what the module reads.
        let writable = decl.writer == Writer::Parameter && decl.reader.is_none();
        let scalar = matches!(
            decl.kind,
            DeclKind::Number { .. } | DeclKind::Integer { .. } | DeclKind::Bool
        );
        (writable && scalar).then(|| Automation {
            cells: self.cells.clone(),
            index,
            kind: decl.kind,
            clamp: decl.clamp,
            event: decl.event,
            whole: decl.whole,
            aliases: None,
        })
    }
}
