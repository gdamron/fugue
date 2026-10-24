use super::*;
use crate::control_request::{Automation, LegacyRefusal, RtValue, TableValue};
use crate::invention::declared::{Declaration, DeclaredSurface, Queued, Route, WriteOptions};
use crate::modules::control_scheduler::SurfaceDirectory;
use std::sync::PoisonError;

use super::fan_out::fanned_out;
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
    /// Held by a declared key's write from its ranking through its
    /// delivery: so a write that loses offline almost always loses before
    /// it records or writes anything (see `DeclaredSurface::admits`), and
    /// the order each payload write carries counts every write before it.
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

    /// Writes payload key `key`, whose alias builds the payload (a
    /// scheduler's resolved schedule), or whose aliases each build their
    /// own (a sample, see `fan_out`). Each alias records `value` first, so
    /// the inner order the payload carries counts what it now writes; a
    /// write that is not applied, failing or losing offline as it is
    /// applied, restores what each recorded. A payload key reaches no
    /// legacy alias, so recording touches nothing else. Under the
    /// writing lock, ranked first (see [`Self::submit`]).
    fn write_payload(
        &self,
        declared: &DeclaredSurface,
        (key, value): (&str, ControlValue),
        options: &WriteOptions,
    ) -> Result<Queued, String> {
        // What each alias records now, whichever key or development wrote
        // it last (a nested development shows every key on it as recorded,
        // see `set_legacy`), and what this one's keys on it read back as:
        // a recording a codec builds from is not what it reads back as.
        let was = (self.aliases(key))
            .map(|(control, surface)| Ok((control, surface, surface.get_control(&control.key)?)))
            .collect::<Result<Vec<_>, String>>()?;
        let shown: Vec<(&str, ControlValue)> = (self.shared_payload_keys(key))
            .filter_map(|shared| Some((shared, self.get_control(shared).ok()?)))
            .collect();
        self.set_legacy(key, value.clone())?;
        let written = match self.prepare_payload(key, &value) {
            Some(prepared) => prepared
                .and_then(|(payload, shown)| declared.submit_payload(key, payload, shown, options)),
            None => declared.submit(key, value, options),
        };
        // Every effect made before delivery is undone here, and only here.
        if written.as_ref().map_or(true, |queued| queued.lost) {
            for (control, surface, was) in was {
                let _ = surface.set_legacy(&control.key, was);
            }
            if let Some(declared) = &self.declared {
                for (shared, value) in shown {
                    declared.show_payload(shared, value);
                }
            }
        }
        written
    }

    /// Each control `key` aliases, with its module's surface.
    fn aliases<'a>(
        &'a self,
        key: &'a str,
    ) -> impl Iterator<Item = (&'a AliasedControl, &'a ControlSurfaceInstance)> {
        (self.controls.iter())
            .filter(move |entry| entry.meta.key == key)
            .filter_map(|control| Some((control, self.surfaces.get(&control.module_id)?)))
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
        // Ranked first: offline, a write that loses to a higher-ranked one
        // at this sample loses before it records or writes anything. The
        // writing lock keeps the ranking standing against every other
        // front door until the write is delivered; only a request log's
        // replay can still take the control in between, and a write it
        // overtakes loses as it is applied. The declared part then goes
        // first: its delivery can still fail (a full request queue, a
        // module being installed) or lose so, and a write not applied must
        // leave the legacy aliases as they were. Once it applies the legacy
        // aliases, already checked, take the value. A payload's one alias
        // records it before delivery, and `write_payload` undoes that.
        if let Some(declared) = self.declared(key) {
            self.validate_control(key, &value, &self.surfaces)?;
            let _writing = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
            if !declared.admits(key, options) {
                return Ok(Queued {
                    lost: true,
                    ..Queued::default()
                });
            }
            // A payload only its alias's own surface can build (a
            // scheduler's resolved schedule) is built there and handed on.
            if declared.is_payload(key) {
                return self.write_payload(declared, (key, value), options);
            }
            let queued = declared.submit(key, value.clone(), options)?;
            // The legacy aliases take the value as the write is made, not
            // as its declared part applies: until their types migrate, a
            // live write that later loses on the audio thread leaves them
            // playing it.
            if !queued.lost {
                self.set_legacy(key, value)?;
            }
            return Ok(queued);
        }
        self.set_legacy(key, value).map(|()| Queued::default())
    }

    /// Built by the alias a payload key reaches, when its surface builds
    /// its own payloads, or by each of its aliases, packed (see
    /// `fan_out`), and wrapped with the inner order it leaves (see
    /// `retarget`): each alias records what it was given first.
    #[allow(private_interfaces)]
    fn prepare_payload(
        &self,
        key: &str,
        value: &ControlValue,
    ) -> Option<Result<(Payload, ControlValue), String>> {
        if !self.declared(key)?.is_payload(key) {
            return None;
        }
        let built = (self.aliases(key))
            .map(|(control, surface)| surface.prepare_payload(&control.key, value))
            .collect::<Option<Vec<_>>>()?;
        let built = built.into_iter().collect::<Result<Vec<_>, String>>();
        Some(built.and_then(|built| {
            let mut built = built.into_iter();
            let (first, shown) = built
                .next()
                .ok_or_else(|| format!("Unknown control: {key}"))?;
            let rest: Vec<Payload> = built.map(|(payload, _)| payload).collect();
            let payload = match rest.is_empty() {
                true => first,
                false => fanned_out(std::iter::once(first).chain(rest).collect()),
            };
            let order = self.order.compute(&self.surfaces);
            Ok((reordered(payload, order), shown))
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

    fn writes_legacy(&self, key: &str) -> bool {
        let reaches = |control: &&AliasedControl| {
            (self.surfaces.get(&control.module_id))
                .is_some_and(|inner| inner.writes_legacy(&control.key))
        };
        self.controls
            .iter()
            .filter(|entry| entry.meta.key == key)
            .any(|control| reaches(&control))
    }

    fn follow_declared(&self, key: &str, value: RtValue) -> Result<(), String> {
        let value = self
            .declared(key)
            .and_then(|declared| declared.value_of(key, value));
        match value {
            Some(value) if self.writes_legacy(key) => self.set_legacy(key, value),
            _ => Ok(()),
        }
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
