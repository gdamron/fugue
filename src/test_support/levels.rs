//! A two-channel mixer's controls for a scheduler's tests: `level.N`, from
//! 0 to 2, unity at first, read as the scheduler's automation leaves them,
//! before any module applies them.

use std::sync::Arc;

use crate::modules::mixer::MixerFactory;
use crate::{ControlSurface, ModuleFactory};

/// A clone shares the levels.
#[derive(Clone)]
pub(crate) struct Levels(Arc<dyn ControlSurface + Send + Sync>);

impl Levels {
    pub(crate) fn new() -> Self {
        let config = serde_json::json!({ "channel_count": 2 });
        let built = MixerFactory.build(48_000, &config).unwrap();
        Self(built.control_surface.unwrap())
    }

    pub(crate) fn surface(&self) -> Arc<dyn ControlSurface + Send + Sync> {
        self.0.clone()
    }

    /// Channel `channel`'s level as automation last wrote it.
    pub(crate) fn level(&self, channel: usize) -> f32 {
        let automation = self.0.automation(&format!("level.{channel}")).unwrap();
        automation.current().unwrap()
    }

    /// Writes channel `channel`'s level as automation does.
    pub(crate) fn set_level(&self, channel: usize, level: f32) {
        let automation = self.0.automation(&format!("level.{channel}")).unwrap();
        automation.write_number(level);
    }
}
