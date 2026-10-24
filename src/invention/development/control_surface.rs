use super::*;
use crate::control_request::{Automation, LegacyRefusal, TableValue};
use crate::invention::declared::{Declaration, DeclaredSurface, Queued, Route, WriteOptions};
use crate::modules::control_scheduler::SurfaceDirectory;
use std::sync::PoisonError;

use super::retarget::{reordered, InnerOrder};

pub(super) struct AliasedControl {
    pub(super) meta: ControlMeta,
    pub(super) module_id: String,
    pub(super) key: String,
}

/// The control surface a development exposes to its outer document.
///
/// The development's internal runtime is consumed when the development is
/// built, but its control-surface directory is kept here for the
/// development's lifetime: an inner `control_scheduler` holds only a weak
/// reference to it, and resolves every new `schedule` through it.
///
/// `surfaces` is a copy of that directory's contents, used for every lookup,
/// so reads and writes never lock the directory (an outer scheduler may
/// write an exposed control from the audio thread). The copy cannot drift:
/// the internal runtime that could add or remove inner modules is gone, and
/// nothing here mutates the directory, so its contents are fixed from build
/// on. Validation and the inner setters therefore resolve against the same
/// directory.
pub(super) struct DevelopmentControlSurface {
    pub(super) controls: Vec<AliasedControl>,
    pub(super) surfaces: IndexMap<String, ControlSurfaceInstance>,
    /// Keeps the internal directory alive for inner schedulers. Locked only
    /// once, at build; held after that only for its ownership, so never
    /// touched on the audio thread.
    #[allow(dead_code)]
    pub(super) directory: SurfaceDirectory,
    /// The exposed controls reaching declared inner controls (see
    /// `declared_controls`): written as the development's own requests.
    pub(super) declared: Option<DeclaredSurface>,
    /// What a payload's inner order is computed from (see `retarget`).
    order: InnerOrder,
    /// Held from a payload's recording through its write, so the order
    /// each write carries counts every write before it.
    writing: Mutex<()>,
}

impl DevelopmentControlSurface {
    /// Builds the surface over `directory`, the development's internal
    /// runtime directory, retaining it (see the type docs).
    pub(super) fn new(
        definition: &Invention,
        directory: SurfaceDirectory,
        declared: Option<DeclaredSurface>,
        order: InnerOrder,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let surfaces = directory.lock().unwrap().clone();
        let mut controls = Vec::with_capacity(definition.controls.len());

        for control in &definition.controls {
            let surface = surfaces
                .get(&control.module)
                .ok_or_else(|| format!("Unknown control module: {}", control.module))?;
            let source = surface
                .controls()
                .into_iter()
                .find(|meta| meta.key == control.control)
                .ok_or_else(|| {
                    format!(
                        "Unknown control '{}' on module '{}'",
                        control.control, control.module
                    )
                })?;

            controls.push(AliasedControl {
                meta: ControlMeta {
                    key: control.name.clone(),
                    description: source.description,
                    default: source.default,
                    kind: source.kind,
                },
                module_id: control.module.clone(),
                key: control.control.clone(),
            });
        }

        Ok(Self {
            controls,
            surfaces,
            directory,
            declared,
            order,
            writing: Mutex::new(()),
        })
    }

    /// The declared part, when `key` is one of its controls.
    fn declared(&self, key: &str) -> Option<&DeclaredSurface> {
        let declared = self.declared.as_ref()?;
        declared.declares(key).then_some(declared)
    }

    /// The payload keys reaching an inner control `key` reaches, `key`
    /// among them when it is one.
    fn shared_payload_keys<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> {
        let reached = move |entry: &AliasedControl| {
            (self.controls.iter()).any(|aliased| {
                aliased.meta.key == key
                    && aliased.module_id == entry.module_id
                    && aliased.key == entry.key
            })
        };
        let mut seen = std::collections::HashSet::new();
        (self.controls.iter())
            .filter(move |entry| reached(entry))
            .map(|entry| entry.meta.key.as_str())
            .filter(move |shared| seen.insert(*shared))
            .filter(|shared| self.declared(shared).is_some_and(|d| d.is_payload(shared)))
    }

    fn lookup(&self, key: &str) -> Result<(&AliasedControl, &ControlSurfaceInstance), String> {
        let control = self
            .controls
            .iter()
            .find(|entry| entry.meta.key == key)
            .ok_or_else(|| format!("Unknown control: {}", key))?;
        let surface = self
            .surfaces
            .get(&control.module_id)
            .ok_or_else(|| format!("Unknown control module: {}", control.module_id))?;
        Ok((control, surface))
    }

    /// Writes payload key `key`, whose one alias builds the payload (a
    /// scheduler's resolved schedule). The alias records `value` first, so
    /// the inner order the payload carries counts what it now writes; a
    /// write that fails restores what it recorded. A payload key reaches
    /// one alias and no legacy one, so recording touches nothing else.
    fn write_payload(
        &self,
        declared: &DeclaredSurface,
        key: &str,
        value: ControlValue,
    ) -> Result<(), String> {
        let _writing = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        // What the alias records now, whichever key or development wrote
        // it last (a nested development shows every key on it as recorded,
        // see `set_legacy`), and what this one's keys on it read back as:
        // a recording a codec builds from is not what it reads back as.
        let (control, surface) = self.lookup(key)?;
        let was = surface.get_control(&control.key)?;
        let shown: Vec<(&str, ControlValue)> = (self.shared_payload_keys(key))
            .filter_map(|shared| Some((shared, self.get_control(shared).ok()?)))
            .collect();
        self.set_legacy(key, value.clone())?;
        let written = match self.prepare_payload(key, &value) {
            Some(prepared) => {
                prepared.and_then(|(payload, shown)| declared.write_payload(key, payload, shown))
            }
            None => declared.set_control(key, value),
        };
        if written.is_err() {
            let _ = self.set_legacy(key, was);
            if let Some(declared) = &self.declared {
                for (shared, value) in shown {
                    declared.show_payload(shared, value);
                }
            }
        }
        written
    }
}

impl ControlSurface for DevelopmentControlSurface {
    fn controls(&self) -> Vec<ControlMeta> {
        // A key may alias several internal controls (an explicit fan-out);
        // list it once.
        let mut seen = std::collections::HashSet::new();
        self.controls
            .iter()
            .filter(|entry| seen.insert(entry.meta.key.as_str()))
            .map(|entry| {
                let mut meta = entry.meta.clone();
                // A payload key lists what it was last written as.
                if let Some(declared) = self.declared(&meta.key) {
                    if declared.is_payload(&meta.key) {
                        if let Ok(shown) = declared.get_control(&meta.key) {
                            meta.default = shown;
                        }
                    }
                }
                meta
            })
            .collect()
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        // Until the development runs, a declared control's writes wait in
        // its own cells; after, its first alias holds what was applied.
        // A payload key is read back from what it was last written as.
        if let Some(declared) = self.declared(key) {
            if declared.is_building() || declared.is_payload(key) {
                return declared.get_control(key);
            }
        }
        let (control, surface) = self.lookup(key)?;
        surface.get_control(&control.key)
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
        // A development may list the same key several times to fan a control
        // out across internal modules (e.g. one `decay` reaching every voice
        // of a bank); apply the write to every aliased target. Aliases on
        // the legacy path are written here; declared ones take it as one
        // request for the development. Every alias of a declared key is
        // checked first, so a value one of them cannot hold changes none of
        // them. A key on the legacy path alone keeps its old, unchecked
        // fan-out: a scheduler still writes it from the audio thread, where
        // checking would allocate.
        //
        // The declared part goes first: its delivery can still fail (a full
        // request queue, a module being installed), and a failed write must
        // leave the legacy aliases as they were. Once it is on its way the
        // legacy aliases, already checked, take the value.
        if let Some(declared) = self.declared(key) {
            self.validate_control(key, &value, &self.surfaces)?;
            // A payload only its alias's own surface can build (a
            // scheduler's resolved schedule) is built there and handed on.
            if declared.is_payload(key) {
                return self
                    .write_payload(declared, key, value)
                    .map(|()| Queued::default());
            }
            let queued = declared.submit(key, value.clone(), options)?;
            self.set_legacy(key, value)?;
            return Ok(queued);
        }
        self.set_legacy(key, value).map(|()| Queued::default())
    }

    /// Built by the one alias a payload key reaches, when its surface
    /// builds its own payloads, and wrapped with the inner order it leaves
    /// (see `retarget`): the alias records what it was given first.
    #[allow(private_interfaces)]
    fn prepare_payload(
        &self,
        key: &str,
        value: &ControlValue,
    ) -> Option<Result<(Payload, ControlValue), String>> {
        if !self.declared(key)?.is_payload(key) {
            return None;
        }
        let (control, surface) = self.lookup(key).ok()?;
        let prepared = surface.prepare_payload(&control.key, value)?;
        Some(prepared.map(|(payload, shown)| {
            let order = self.order.compute(&self.surfaces);
            (reordered(payload, order), shown)
        }))
    }

    /// Writes `key`'s aliases still on the legacy path, and theirs in a
    /// nested development, on this (control) thread.
    fn set_legacy(&self, key: &str, value: ControlValue) -> Result<(), String> {
        let mut found = false;
        for control in self.controls.iter().filter(|entry| entry.meta.key == key) {
            let surface = self
                .surfaces
                .get(&control.module_id)
                .ok_or_else(|| format!("Unknown control module: {}", control.module_id))?;
            surface.set_legacy(&control.key, value.clone())?;
            found = true;
        }
        // A payload written through an outer development reads back as
        // written through every key on the same inner control, as its
        // alias does.
        if let Some(declared) = &self.declared {
            for shared in self.shared_payload_keys(key) {
                declared.show_payload(shared, value.clone());
            }
        }
        if found {
            Ok(())
        } else {
            Err(format!("Unknown control: {}", key))
        }
    }

    /// Writes every alias of `key`, a key on the legacy path alone (a
    /// schedule writes no other), through its own surface's scheduled
    /// setter: allocation-, free- and lock-free, as theirs are. An alias
    /// refusing the write leaves the others written, and the first refusal
    /// is returned.
    #[allow(private_interfaces)]
    fn set_scheduled(&self, key: &str, value: TableValue) -> Result<(), LegacyRefusal> {
        let mut found = false;
        let mut refused = None;
        for control in self.controls.iter().filter(|entry| entry.meta.key == key) {
            found = true;
            let written = (self.surfaces.get(&control.module_id))
                .ok_or(LegacyRefusal::Unsupported)
                .and_then(|surface| surface.set_scheduled(&control.key, value));
            if let Err(refusal) = written {
                refused.get_or_insert(refusal);
            }
        }
        match (found, refused) {
            (true, None) => Ok(()),
            (true, Some(refusal)) => Err(refusal),
            (false, _) => Err(LegacyRefusal::Unsupported),
        }
    }

    /// Reads `key`'s first alias, as [`Self::get_control`] does a key on
    /// the legacy path.
    #[allow(private_interfaces)]
    fn get_scheduled(&self, key: &str) -> Result<f32, LegacyRefusal> {
        let control = (self.controls.iter())
            .find(|entry| entry.meta.key == key)
            .ok_or(LegacyRefusal::Unsupported)?;
        let surface = (self.surfaces.get(&control.module_id)).ok_or(LegacyRefusal::Unsupported)?;
        surface.get_scheduled(&control.key)
    }

    fn bind(&self, route: Route, module: &mut dyn Module) {
        if let Some(declared) = &self.declared {
            declared.bind(route, module);
        }
    }

    fn activate(&self, route: Route) {
        if let Some(declared) = &self.declared {
            declared.activate(route);
        }
    }

    fn retire(&self) {
        if let Some(declared) = &self.declared {
            declared.retire();
        }
    }

    fn scheduled_writes_refused(&self) -> u64 {
        let inner = self.surfaces.values();
        inner
            .map(|surface| surface.scheduled_writes_refused())
            .sum()
    }

    fn declares(&self, key: &str) -> bool {
        self.declared(key).is_some()
    }

    /// Only for a key every alias of which automation can write: a key
    /// still reaching a legacy alias cannot be scheduled (its setter would
    /// lock on the audio thread). A write lands in every alias's own slot
    /// (see [`Automation::aliases`]), so keys sharing an inner control
    /// apply in the order they were written, and a ramp starts from what
    /// the first alias holds or is about to, whichever key (or inner
    /// scheduler) moved it.
    fn automation(&self, key: &str) -> Option<Automation> {
        let mut automation = self.declared(key)?.automation(key)?;
        let aliases = self
            .controls
            .iter()
            .filter(|entry| entry.meta.key == key)
            .map(|control| {
                self.surfaces
                    .get(&control.module_id)?
                    .automation(&control.key)
            })
            .collect::<Option<Vec<_>>>()?;
        automation.event |= aliases.iter().any(|alias| alias.event);
        automation.aliases = Some(aliases.into());
        Some(automation)
    }

    /// Its declared part, to a development nesting this one: the nesting
    /// development reaches it by request, and the rest by
    /// [`ControlSurface::set_legacy`].
    fn declaration(&self, key: &str) -> Option<Declaration> {
        self.declared(key)?.declaration(key)
    }

    /// Validates the write against every internal control `key` aliases.
    /// Inner surfaces resolve against the development's own directory, as
    /// their setters do, never against the outer candidate's.
    fn validate_control(
        &self,
        key: &str,
        value: &ControlValue,
        _surfaces: &crate::traits::ControlSurfaceMap,
    ) -> Result<(), String> {
        let mut found = false;
        for control in self.controls.iter().filter(|entry| entry.meta.key == key) {
            let surface = self
                .surfaces
                .get(&control.module_id)
                .ok_or_else(|| format!("Unknown control module: {}", control.module_id))?;
            surface.validate_control(&control.key, value, &self.surfaces)?;
            found = true;
        }
        if found {
            Ok(())
        } else {
            Err(format!("Unknown control: {}", key))
        }
    }
}
