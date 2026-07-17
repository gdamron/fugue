//! Sample player module for audio file playback.

use std::any::Any;
use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::Module;

pub use self::controls::SamplePlayerControls;

mod controls;
mod inputs;
mod outputs;

pub struct SamplePlayerFactory;

impl ModuleFactory for SamplePlayerFactory {
    fn type_id(&self) -> &'static str {
        "sample_player"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        // `asset` is the hybrid reference form (package ref string or
        // `{ "path": ... }` object); `source` remains the plain string form.
        // Invention loads resolve `asset` to a concrete path before this
        // factory runs; a module added live may still carry a package ref,
        // which `set_source` resolves through the package cache.
        let asset = config
            .get("asset")
            .map(|value| {
                serde_json::from_value::<crate::pkg::AudioAssetRef>(value.clone())
                    .map_err(|err| format!("invalid asset reference {}: {}", value, err))
            })
            .transpose()?;
        let source = config.get("source").and_then(|value| value.as_str());
        if asset.is_some() && source.is_some() {
            return Err("config accepts either 'asset' or 'source', not both".into());
        }
        let asset = asset.map(|asset| match asset {
            crate::pkg::AudioAssetRef::Text(text) => text,
            crate::pkg::AudioAssetRef::Local { path } => path,
        });
        let source = asset.as_deref().or(source);
        let play = config.get("play").and_then(|value| value.as_bool());
        let loop_enabled = config.get("loop_enabled").and_then(|value| value.as_bool());
        let controls = SamplePlayerControls::new(sample_rate, source, play, loop_enabled)?;
        let player = SamplePlayer::new_with_controls(controls.clone());

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(player)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
    }
}

pub struct SamplePlayer {
    ctrl: SamplePlayerControls,
    inputs: inputs::SamplePlayerInputs,
    outputs: outputs::SamplePlayerOutputs,
    sample: Option<Arc<controls::SampleData>>,
    position: f64,
    playing: bool,
    last_play_input: f32,
    last_play_trigger: u64,
    last_control_play: bool,
    pending_start_gate: bool,
}

impl SamplePlayer {
    pub fn new_with_controls(controls: SamplePlayerControls) -> Self {
        Self {
            ctrl: controls,
            inputs: inputs::SamplePlayerInputs::new(),
            outputs: outputs::SamplePlayerOutputs::new(),
            sample: None,
            position: 0.0,
            playing: false,
            last_play_input: 0.0,
            last_play_trigger: 0,
            last_control_play: false,
            pending_start_gate: false,
        }
    }

    fn restart(&mut self) {
        self.position = 0.0;
        self.playing = self
            .sample
            .as_ref()
            .map(|sample| sample.len() > 0)
            .unwrap_or(false);
        self.pending_start_gate = self.playing;
    }
}

impl Module for SamplePlayer {
    fn name(&self) -> &str {
        "SamplePlayer"
    }

    fn process(&mut self, frames: usize) -> bool {
        // Control-rate state read once per block.
        let (control_play, play_trigger, loop_control, pending_sample) = {
            let mut shared = self.ctrl.shared.lock().unwrap();
            (
                shared.play,
                shared.play_trigger,
                shared.loop_enabled,
                shared.pending_sample.take(),
            )
        };

        if let Some(sample) = pending_sample {
            self.sample = Some(sample);
            self.position = 0.0;
            self.playing = control_play;
            self.pending_start_gate = self.playing;
        }

        for i in 0..frames {
            let gate_rising = self.inputs.play(i) > 0.5 && self.last_play_input <= 0.5;
            if play_trigger != self.last_play_trigger {
                self.last_play_trigger = play_trigger;
                self.restart();
            } else if gate_rising {
                self.restart();
            } else if !control_play && self.last_control_play {
                self.playing = false;
                self.position = 0.0;
                self.pending_start_gate = false;
            }

            self.last_control_play = control_play;

            let loop_enabled = self.inputs.loop_enabled(i, loop_control);
            let mut start_gate = 0.0;
            let mut end_gate = 0.0;
            let mut left = 0.0;
            let mut right = 0.0;

            if self.pending_start_gate && self.playing {
                start_gate = 1.0;
                self.pending_start_gate = false;
            }

            if let Some(sample) = &self.sample {
                let len = sample.len();
                if self.playing && len > 0 {
                    let (l, r) = sample.sample_at(self.position);
                    left = l;
                    right = r;

                    let pitch = self.inputs.pitch(i, self.ctrl.pitch_ratio()).max(1e-4);
                    self.position += pitch as f64;

                    if self.position >= len as f64 {
                        end_gate = 1.0;
                        if loop_enabled {
                            // `%=` keeps the fractional read head bounded without
                            // accumulating drift across loops.
                            self.position %= len as f64;
                            self.pending_start_gate = true;
                        } else {
                            self.playing = false;
                            self.position = 0.0;
                            if control_play {
                                self.ctrl.set_play(false);
                                self.last_control_play = false;
                            }
                        }
                    }
                }
            }

            self.outputs.set(i, left, right, start_gate, end_gate);
            self.last_play_input = self.inputs.play(i);
        }
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        self.inputs.block_mut(index)
    }

    fn output_block(&self, index: usize) -> &[f32] {
        self.outputs.block(index)
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    fn set_input_connected(&mut self, index: usize, connected: bool) {
        self.inputs.set_connected(index, connected);
    }
}

#[cfg(test)]
mod tests;
