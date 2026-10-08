use super::*;
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
}

impl DevelopmentControlSurface {
    /// Builds the surface over `directory`, the development's internal
    /// runtime directory, retaining it (see the type docs).
    pub(super) fn new(
        definition: &Invention,
        directory: SurfaceDirectory,
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
        })
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
        let (control, surface) = self.lookup(key)?;
        surface.get_control(&control.key)
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        // A development may list the same key several times to fan a control
        // out across internal modules (e.g. one `decay` reaching every voice
        // of a bank); apply the write to every aliased target.
        let mut found = false;
        for control in self.controls.iter().filter(|entry| entry.meta.key == key) {
            let surface = self
                .surfaces
                .get(&control.module_id)
                .ok_or_else(|| format!("Unknown control module: {}", control.module_id))?;
            surface.set_control(&control.key, value.clone())?;
            found = true;
        }
        if found {
            Ok(())
        } else {
            Err(format!("Unknown control: {}", key))
        }
    }

    /// Whether any alias of `key` is a declared control. Automation cannot
    /// write through a development yet, so a schedule targeting such a key
    /// is refused rather than reach a declared setter from the audio thread.
    fn declares(&self, key: &str) -> bool {
        self.controls
            .iter()
            .filter(|entry| entry.meta.key == key)
            .any(|entry| {
                let surface = self.surfaces.get(&entry.module_id);
                surface.is_some_and(|surface| surface.declares(&entry.key))
            })
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
