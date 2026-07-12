use super::*;

pub(super) struct AliasedControl {
    pub(super) meta: ControlMeta,
    pub(super) module_id: String,
    pub(super) key: String,
}

pub(super) struct DevelopmentControlSurface {
    pub(super) controls: Vec<AliasedControl>,
    pub(super) surfaces: IndexMap<String, ControlSurfaceInstance>,
}

impl DevelopmentControlSurface {
    pub(super) fn new(
        definition: &Invention,
        surfaces: &IndexMap<String, ControlSurfaceInstance>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
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
            surfaces: surfaces.clone(),
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
}
