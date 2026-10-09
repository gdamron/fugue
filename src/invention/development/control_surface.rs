use super::*;
use crate::control_request::Automation;
use crate::invention::declared::{Declaration, DeclaredSurface, Route};
use crate::modules::control_scheduler::SurfaceDirectory;

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
}

impl DevelopmentControlSurface {
    /// Builds the surface over `directory`, the development's internal
    /// runtime directory, retaining it (see the type docs).
    pub(super) fn new(
        definition: &Invention,
        directory: SurfaceDirectory,
        declared: Option<DeclaredSurface>,
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
                    key: control.key.clone(),
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
        })
    }

    /// The declared part, when `key` is one of its controls.
    fn declared(&self, key: &str) -> Option<&DeclaredSurface> {
        let declared = self.declared.as_ref()?;
        declared.declares(key).then_some(declared)
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
}

impl ControlSurface for DevelopmentControlSurface {
    fn controls(&self) -> Vec<ControlMeta> {
        // A key may alias several internal controls (an explicit fan-out);
        // list it once.
        let mut seen = std::collections::HashSet::new();
        self.controls
            .iter()
            .filter(|entry| seen.insert(entry.meta.key.as_str()))
            .map(|entry| entry.meta.clone())
            .collect()
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        // Until the development runs, a declared control's writes wait in
        // its own cells; after, its first alias holds what was applied.
        if let Some(declared) = self.declared(key) {
            if declared.is_building() {
                return declared.get_control(key);
            }
        }
        let (control, surface) = self.lookup(key)?;
        surface.get_control(&control.key)
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
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
            declared.set_control(key, value.clone())?;
        }
        self.set_legacy(key, value)
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
        if found {
            Ok(())
        } else {
            Err(format!("Unknown control: {}", key))
        }
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
