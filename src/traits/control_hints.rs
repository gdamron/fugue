//! Presentation hints for number controls.
//!
//! Hints say how an editor should show and step a number (its unit, how
//! its range maps onto a slider, how far a key press moves it, how many
//! decimals it shows). They are metadata only: no hint changes what a
//! control accepts, how it is clamped, or what the module does with it.
//! Every hint is optional, and an absent one is left out of the wire form,
//! so clients that predate hints read the same listing they always did.

use serde::{Deserialize, Serialize};

/// How an editor maps a number's range onto its travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ControlScale {
    /// Equal travel for equal differences (levels, fractions, tempo).
    Linear,
    /// Equal travel for equal ratios (frequencies and times, where an
    /// octave or a doubling is the musical step). Only declared on a range
    /// whose minimum is above zero.
    Log,
}

/// What a number stands for, when an editor can show it better than as a
/// plain number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum NumberFormat {
    /// A MIDI note number, which an editor may show by its name: 60 is C4,
    /// 69 is A4 (440 Hz).
    Note,
}

/// Optional presentation hints for a number control (see the module docs).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct NumberHints {
    /// The unit the value is in, shown after it (`"Hz"`, `"s"`, `"BPM"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// How the range maps onto an editor's travel; linear when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<ControlScale>,
    /// A keyboard step in the value's unit, for a linear editor: 1 for a
    /// whole-number control.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f32>,
    /// Decimal places worth showing: 0 for a whole-number control.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precision: Option<u8>,
    /// What the number stands for, such as a note, when an editor can name
    /// it; a plain number when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<NumberFormat>,
}

impl NumberHints {
    /// Whether no hint is set, so the listing leaves `hints` out.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// The editor a module type prefers for a control. Editors otherwise
/// choose by the control's kind; a preference never changes what the
/// control accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ControlWidget {
    /// A compact rotary control, for a number.
    Knob,
    /// A linear fader, for a number set finely.
    Slider,
    /// A number field with minus and plus, for a whole number.
    Stepper,
    /// Side-by-side buttons, one per option, for a short choice.
    Segmented,
    /// A drop-down list, for a choice.
    Menu,
}
