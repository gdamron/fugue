//! A test module whose builds follow a script: pass, fail, hide its control,
//! or run a hook (another edit landing mid-batch, say). It also logs every
//! control write it receives, and refuses them all when its config carries
//! an `error`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::{ControlMeta, ControlSurface, ControlValue, ModuleRegistry};

/// The scripted module's type name.
pub(crate) const SCRIPTED: &str = "scripted";

/// What one build does. Builds without a step pass.
pub(crate) enum Step {
    /// Fails to build.
    Fail,
    /// Builds a module whose `level` control cannot be read or listed, so a
    /// schedule targeting it does not resolve.
    Hide,
    /// Runs the hook, then builds normally.
    Run(Box<dyn FnOnce() + Send>),
    /// Builds a module that refuses every write with this reason, though its
    /// config does not say so: a running module that has drifted from the
    /// document it was built from.
    Refuse(String),
}

/// The factory, shared with the test that scripts it.
#[derive(Clone, Default)]
pub(crate) struct Scripted {
    steps: Arc<Mutex<VecDeque<Option<Step>>>>,
    writes: Arc<Mutex<Vec<(String, ControlValue)>>>,
    builds: Arc<Mutex<usize>>,
}

impl Scripted {
    /// Queues what the next unscripted build does; `None` passes.
    pub(crate) fn then(&self, step: Option<Step>) -> &Self {
        self.steps.lock().unwrap().push_back(step);
        self
    }

    /// Every control write any instance received, in order.
    pub(crate) fn writes(&self) -> Vec<(String, ControlValue)> {
        self.writes.lock().unwrap().clone()
    }

    /// Builds made so far.
    pub(crate) fn builds(&self) -> usize {
        *self.builds.lock().unwrap()
    }

    /// The default registry with this factory registered.
    pub(crate) fn registry(&self) -> ModuleRegistry {
        let mut registry = ModuleRegistry::default();
        registry.register(self.clone());
        registry
    }
}

struct Controls {
    level: Mutex<f32>,
    hidden: bool,
    refusal: Option<String>,
    writes: Arc<Mutex<Vec<(String, ControlValue)>>>,
}

impl ControlSurface for Controls {
    fn controls(&self) -> Vec<ControlMeta> {
        if self.hidden {
            return Vec::new();
        }
        vec![ControlMeta::number("level", "Level")]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "level" if !self.hidden => Ok(ControlValue::Number(*self.level.lock().unwrap())),
            _ => Err(format!("Unknown control: {key}")),
        }
    }

    /// Refuses what the setter refuses: every write when the module was
    /// built to refuse them, and anything but a number for `level`.
    fn validate_control(
        &self,
        key: &str,
        value: &ControlValue,
        _surfaces: &crate::traits::ControlSurfaceMap,
    ) -> Result<(), String> {
        if let Some(refusal) = &self.refusal {
            return Err(refusal.clone());
        }
        match key {
            "level" if !self.hidden => value.as_number().map(drop),
            _ => Err(format!("Unknown control: {key}")),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        self.writes
            .lock()
            .unwrap()
            .push((key.to_string(), value.clone()));
        if let Some(refusal) = &self.refusal {
            return Err(refusal.clone());
        }
        *self.level.lock().unwrap() = value.as_number()?;
        Ok(())
    }
}

impl crate::ModuleFactory for Scripted {
    fn type_id(&self) -> &'static str {
        SCRIPTED
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<crate::ModuleBuildResult, Box<dyn std::error::Error>> {
        *self.builds.lock().unwrap() += 1;
        let step = self.steps.lock().unwrap().pop_front().flatten();
        let mut refusal = config
            .get("error")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let hidden = match step {
            Some(Step::Fail) => return Err("scripted build failure".into()),
            Some(Step::Hide) => true,
            Some(Step::Run(hook)) => {
                hook();
                false
            }
            Some(Step::Refuse(reason)) => {
                refusal = Some(reason);
                false
            }
            None => false,
        };
        // Any port-less module will do; only the surface matters here.
        let module = ModuleRegistry::default()
            .build("code", sample_rate, &serde_json::json!({}))?
            .module;
        let level = config.get("level").and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok(crate::ModuleBuildResult {
            module,
            handles: Vec::new(),
            control_surface: Some(Arc::new(Controls {
                level: Mutex::new(level as f32),
                hidden,
                refusal,
                writes: self.writes.clone(),
            })),
            sink: None,
        })
    }
}
