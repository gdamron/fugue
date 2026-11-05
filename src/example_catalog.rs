use std::path::{Path, PathBuf};

pub const EXAMPLES_DIR: &str = "examples";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExampleDefinition {
    pub file_name: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
}

impl ExampleDefinition {
    pub fn path(self) -> PathBuf {
        Path::new(EXAMPLES_DIR).join(self.file_name)
    }
}

const PLAYABLE_EXAMPLES: [ExampleDefinition; 28] = [
    ExampleDefinition {
        file_name: "simple_tone.json",
        title: "Simple Tone",
        summary: "Minimal ADSR-shaped 440Hz tone.",
    },
    ExampleDefinition {
        file_name: "modular_adsr_melody.json",
        title: "Modular ADSR Melody",
        summary: "Clocked melody with ADSR, VCA, and reverb.",
    },
    ExampleDefinition {
        file_name: "filter_envelope.json",
        title: "Filter Envelope",
        summary: "Subtractive synth patch with envelope-controlled cutoff.",
    },
    ExampleDefinition {
        file_name: "filter_lfo_wah.json",
        title: "Filter LFO Wah",
        summary: "LFO-driven filter sweep for an auto-wah effect.",
    },
    ExampleDefinition {
        file_name: "lfo_vibrato.json",
        title: "LFO Vibrato",
        summary: "Pitch modulation with a sine LFO.",
    },
    ExampleDefinition {
        file_name: "lfo_tremolo_sync.json",
        title: "LFO Tremolo Sync",
        summary: "Phase-synced tremolo that resets on each note.",
    },
    ExampleDefinition {
        file_name: "mixer_voices.json",
        title: "Mixer Voices",
        summary: "Layered oscillators mixed into one voice.",
    },
    ExampleDefinition {
        file_name: "noise_surf_and_hats.json",
        title: "Noise Surf and Hats",
        summary: "Pink-noise surf under a filter sweep, with a white-noise hi-hat.",
    },
    ExampleDefinition {
        file_name: "delay_dotted_echo.json",
        title: "Dotted Eighth Echo",
        summary: "A plucked line through a dotted-eighth delay timed by the clock.",
    },
    ExampleDefinition {
        file_name: "drive_bass_and_crushed_bells.json",
        title: "Drive: Bass and Crushed Bells",
        summary: "A bass warmed by asymmetric drive under bit-crushed bells.",
    },
    ExampleDefinition {
        file_name: "limiter_hot_mix.json",
        title: "Limiter: Hot Generative Mix",
        summary: "Generative voices pushed past full scale, held under -1 dBFS by a limiter.",
    },
    ExampleDefinition {
        file_name: "macro_oscillator_pluck.json",
        title: "Macro Oscillator Pluck",
        summary: "A sequenced macro oscillator plucked by its own low-pass gate.",
    },
    ExampleDefinition {
        file_name: "macro_oscillator_folded_bass.json",
        title: "Macro Oscillator Folded Bass",
        summary: "A wavefolded bassline from the macro oscillator's waveshaping model.",
    },
    ExampleDefinition {
        file_name: "macro_oscillator_fm_bells.json",
        title: "Macro Oscillator FM Bells",
        summary: "Inharmonic two-operator FM bells ringing through a long gate decay.",
    },
    ExampleDefinition {
        file_name: "macro_oscillator_formant_choir.json",
        title: "Macro Oscillator Formant Choir",
        summary: "A sung-sounding line from the macro oscillator's grain formant model.",
    },
    ExampleDefinition {
        file_name: "macro_oscillator_additive_organ.json",
        title: "Macro Oscillator Additive Organ",
        summary: "Drawbar-style additive organ held by the gate's level mode.",
    },
    ExampleDefinition {
        file_name: "macro_oscillator_chords_in_key.json",
        title: "Macro Oscillator Chords in Key",
        summary: "A seventh-chord progression in D dorian from a single-note bassline.",
    },
    ExampleDefinition {
        file_name: "macro_oscillator_vowel_chant.json",
        title: "Macro Oscillator Vowel Chant",
        summary: "A low vowel chant from the macro oscillator's formant speech model.",
    },
    ExampleDefinition {
        file_name: "contour_rolling_filters.json",
        title: "Contour Rolling Filters",
        summary: "One contour's four phase-offset outputs open the filters of a chord in turn.",
    },
    ExampleDefinition {
        file_name: "resonator_modal_bells.json",
        title: "Resonator Modal Bells",
        summary: "Struck bell-like modal bodies, four voices ringing on under each other.",
    },
    ExampleDefinition {
        file_name: "step_sequencer.json",
        title: "Step Sequencer",
        summary: "Deterministic bassline built from a 16-step pattern.",
    },
    ExampleDefinition {
        file_name: "chance_groove.json",
        title: "Chance Groove",
        summary: "One chance module plays drums and a lead in D dorian, looping and mutating.",
    },
    ExampleDefinition {
        file_name: "drum_map.json",
        title: "Drum Map Groove",
        summary: "Kick, snare and hats from a navigable map of grooves.",
    },
    ExampleDefinition {
        file_name: "control_scheduler.json",
        title: "Control Scheduler Hairpins",
        summary: "Bassline shaped by scheduled dynamics and a tempo change.",
    },
    ExampleDefinition {
        file_name: "development_file_patch.json",
        title: "Development Patch (File)",
        summary: "Registers a reusable voice from a sibling JSON file.",
    },
    ExampleDefinition {
        file_name: "development_inline_patch.json",
        title: "Development Patch (Inline)",
        summary: "Registers the voice development inline.",
    },
    ExampleDefinition {
        file_name: "developments/voice_library_trio.json",
        title: "Voice Library Trio",
        summary: "Layers piano, marimba, and pad developments through one mixer.",
    },
    ExampleDefinition {
        file_name: "in_c.json",
        title: "In C — Terry Riley",
        summary:
            "13-voice generative performance of Terry Riley's In C with self-progressing voices.",
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExampleCommand {
    Interactive,
    RunByName(String),
    Help,
}

pub fn playable_examples() -> Vec<ExampleDefinition> {
    PLAYABLE_EXAMPLES
        .iter()
        .copied()
        .filter(|example| example.path().is_file())
        .collect()
}

pub fn resolve_example(name: &str) -> Result<ExampleDefinition, String> {
    let full = Path::new(name)
        .strip_prefix(EXAMPLES_DIR)
        .unwrap_or_else(|_| Path::new(name));
    let full = full.to_string_lossy();
    let bare = Path::new(name)
        .file_name()
        .and_then(|file_name| file_name.to_str());

    playable_examples()
        .into_iter()
        .find(|example| {
            example.file_name == full
                || bare.is_some_and(|bare| {
                    example.file_name == bare
                        || Path::new(example.file_name)
                            .file_name()
                            .and_then(|file_name| file_name.to_str())
                            == Some(bare)
                })
        })
        .ok_or_else(|| format!("Unknown example '{name}'.\n\n{}", valid_examples_message()))
}

pub fn parse_example_command<I, S>(args: I) -> Result<ExampleCommand, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    let _program = args.next();

    let Some(first) = args.next() else {
        return Ok(ExampleCommand::Interactive);
    };

    match first.as_ref() {
        "-h" | "--help" => {
            if let Some(extra) = args.next() {
                return Err(format!(
                    "Unexpected argument '{}'.\n\n{}",
                    extra.as_ref(),
                    usage()
                ));
            }
            Ok(ExampleCommand::Help)
        }
        "--example" => {
            let Some(file_name) = args.next() else {
                return Err(format!(
                    "Missing JSON filename after --example.\n\n{}",
                    usage()
                ));
            };
            if let Some(extra) = args.next() {
                return Err(format!(
                    "Unexpected argument '{}'.\n\n{}",
                    extra.as_ref(),
                    usage()
                ));
            }
            Ok(ExampleCommand::RunByName(file_name.as_ref().to_string()))
        }
        other => Err(format!("Unknown argument '{other}'.\n\n{}", usage())),
    }
}

pub fn usage() -> String {
    String::from(
        "Usage:
  cargo run --example examples
  cargo run --example examples -- --example <json-file>
  cargo run --example examples -- --help",
    )
}

pub fn valid_examples_message() -> String {
    let mut message = String::from("Available examples:");

    for example in playable_examples() {
        message.push_str("\n  - ");
        message.push_str(example.file_name);
    }

    message
}

#[cfg(test)]
mod tests {
    use super::{
        parse_example_command, playable_examples, resolve_example, valid_examples_message,
        ExampleCommand,
    };

    #[test]
    fn playable_examples_match_curated_allowlist() {
        let names: Vec<_> = playable_examples()
            .into_iter()
            .map(|example| example.file_name)
            .collect();

        assert_eq!(
            names,
            vec![
                "simple_tone.json",
                "modular_adsr_melody.json",
                "filter_envelope.json",
                "filter_lfo_wah.json",
                "lfo_vibrato.json",
                "lfo_tremolo_sync.json",
                "mixer_voices.json",
                "noise_surf_and_hats.json",
                "delay_dotted_echo.json",
                "drive_bass_and_crushed_bells.json",
                "limiter_hot_mix.json",
                "macro_oscillator_pluck.json",
                "macro_oscillator_folded_bass.json",
                "macro_oscillator_fm_bells.json",
                "macro_oscillator_formant_choir.json",
                "macro_oscillator_additive_organ.json",
                "macro_oscillator_chords_in_key.json",
                "macro_oscillator_vowel_chant.json",
                "contour_rolling_filters.json",
                "resonator_modal_bells.json",
                "step_sequencer.json",
                "chance_groove.json",
                "drum_map.json",
                "control_scheduler.json",
                "development_file_patch.json",
                "development_inline_patch.json",
                "developments/voice_library_trio.json",
                "in_c.json",
            ]
        );
    }

    #[test]
    fn helper_files_are_not_selectable_examples() {
        let names: Vec<_> = playable_examples()
            .into_iter()
            .map(|example| example.file_name)
            .collect();

        assert!(!names.contains(&"development_voice.json"));
        assert!(!valid_examples_message().contains("development_voice.json"));
    }

    #[test]
    fn resolve_example_accepts_bare_filename_and_relative_path() {
        let bare = resolve_example("simple_tone.json").unwrap();
        let with_path = resolve_example("examples/simple_tone.json").unwrap();

        assert_eq!(bare.file_name, "simple_tone.json");
        assert_eq!(with_path.file_name, "simple_tone.json");
    }

    #[test]
    fn resolve_example_accepts_nested_example_paths() {
        let bare = resolve_example("voice_library_trio.json").unwrap();
        let nested = resolve_example("developments/voice_library_trio.json").unwrap();
        let with_examples_prefix =
            resolve_example("examples/developments/voice_library_trio.json").unwrap();

        assert_eq!(bare.file_name, "developments/voice_library_trio.json");
        assert_eq!(nested.file_name, "developments/voice_library_trio.json");
        assert_eq!(
            with_examples_prefix.file_name,
            "developments/voice_library_trio.json"
        );
    }

    #[test]
    fn parse_example_command_defaults_to_interactive() {
        let command = parse_example_command(["examples"]).unwrap();
        assert_eq!(command, ExampleCommand::Interactive);
    }

    #[test]
    fn parse_example_command_supports_direct_run() {
        let command =
            parse_example_command(["examples", "--example", "step_sequencer.json"]).unwrap();

        assert_eq!(
            command,
            ExampleCommand::RunByName(String::from("step_sequencer.json"))
        );
    }

    #[test]
    fn parse_example_command_rejects_unknown_flags() {
        let error = parse_example_command(["examples", "--variant", "file"]).unwrap_err();

        assert!(error.contains("Unknown argument '--variant'"));
        assert!(error.contains("cargo run --example examples -- --example <json-file>"));
    }
}
