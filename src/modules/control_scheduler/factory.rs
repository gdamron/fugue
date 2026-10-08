//! Building control schedulers and attaching them to a runtime's
//! control-surface directory.

use std::sync::Arc;

use super::CONTROL_SCHEDULER_TYPE_ID;
use super::{schedule, ControlScheduler, ControlSchedulerControls, SurfaceDirectory};
use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::module_config::{ConfigKey, ConfigReader};

/// Factory for constructing ControlScheduler modules from configuration.
pub struct ControlSchedulerFactory;

const BPM_SCALE: ConfigKey = ConfigKey::float("bpm_scale");

impl ModuleFactory for ControlSchedulerFactory {
    fn type_id(&self) -> &'static str {
        CONTROL_SCHEDULER_TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        &[BPM_SCALE]
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        // Read even without a tempo map, so a bad value is refused as written.
        let bpm_scale = ConfigReader::new(CONTROL_SCHEDULER_TYPE_ID, config)
            .float(&BPM_SCALE)?
            .unwrap_or(1.0);
        let mut spec =
            schedule::parse_schedule(config.get("schedule").unwrap_or(&serde_json::Value::Null))?;
        // A score tempo map (spliced in via `$asset`) compiles into schedule
        // entries that write a clock's tempo at each change's step boundary.
        if let Some(tempo_map) = config.get("tempo_map").filter(|value| !value.is_null()) {
            let module = config
                .get("tempo_target")
                .and_then(|value| value.as_str())
                .unwrap_or("clock");
            let control = config
                .get("tempo_control")
                .and_then(|value| value.as_str())
                .unwrap_or("bpm");
            spec.extend(schedule::compile_tempo_map(
                tempo_map, module, control, bpm_scale,
            )?);
        }
        let controls = ControlSchedulerControls::new(spec);
        let module = ControlScheduler::new(sample_rate, controls.clone());

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(module)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn std::any::Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
    }
}

/// Attaches a just-built scheduler to the runtime's control-surface
/// directory via its type-erased `controls` handle. Shared by every path
/// that can introduce a scheduler (builder, live add, swap).
pub(crate) fn attach_from_handle(
    module_id: &str,
    handle: Option<&Arc<dyn std::any::Any + Send + Sync>>,
    directory: &SurfaceDirectory,
) -> Result<(), String> {
    let controls = handle
        .and_then(|handle| handle.downcast_ref::<ControlSchedulerControls>())
        .ok_or_else(|| {
            format!(
                "control_scheduler '{}' is missing its controls handle",
                module_id
            )
        })?;
    controls.attach(module_id, directory)
}

/// Names a throwaway scheduler copy that is never attached, via its
/// type-erased `controls` handle (see
/// [`ControlSchedulerControls::name_unattached`]). Does nothing for a module
/// that is not a scheduler.
pub(crate) fn name_unattached_from_handles(
    module_id: &str,
    handles: &[(String, Arc<dyn std::any::Any + Send + Sync>)],
) {
    for (_, handle) in handles {
        if let Some(controls) = handle.downcast_ref::<ControlSchedulerControls>() {
            controls.name_unattached(module_id);
        }
    }
}

/// Like [`attach_from_handle`], but resolves the schedule against `surfaces`
/// (the directory as a pending graph change will leave it), so a scheduler
/// may target a module added in the same change. Nothing outside the new
/// scheduler changes until that change commits.
pub(crate) fn attach_from_handle_resolving(
    module_id: &str,
    handle: Option<&Arc<dyn std::any::Any + Send + Sync>>,
    directory: &SurfaceDirectory,
    surfaces: &schedule::SurfaceMap,
) -> Result<(), String> {
    let controls = handle
        .and_then(|handle| handle.downcast_ref::<ControlSchedulerControls>())
        .ok_or_else(|| {
            format!(
                "control_scheduler '{}' is missing its controls handle",
                module_id
            )
        })?;
    controls.attach_resolving(module_id, directory, surfaces)
}
