//! A control range missing a bound, named by the development that holds it.
//!
//! A derived `Deserialize` reports a range missing a bound as serde's bare
//! "missing field `min`", which names neither the development nor the
//! control. Reading a control keeps its fields as the derive reads them, so
//! a field written twice, an unknown field and a `null` are handled as ever,
//! but a range with a missing or `null` bound parses and the control notes
//! the bound. Where a development's definition is resolved, which knows the
//! development's name, [`refuse_partial_ranges`] fails the load with it; so
//! does a build of a document whose own controls hold one.

use serde::{Deserialize, Deserializer};

use super::{ControlRange, DevelopmentControl, Invention};

/// A range as written, with either bound possibly absent. It is named for
/// the type it becomes, since an error says what struct it expected.
mod written {
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub(super) struct ControlRange {
        pub(super) min: Option<f64>,
        pub(super) max: Option<f64>,
    }
}

impl<'de> Deserialize<'de> for DevelopmentControl {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Written {
            name: String,
            module: String,
            control: String,
            range: Option<written::ControlRange>,
        }
        let Written {
            name,
            module,
            control,
            range,
        } = Written::deserialize(deserializer)?;
        let (range, range_missing) = match range {
            None => (None, None),
            Some(written::ControlRange {
                min: Some(min),
                max: Some(max),
            }) => (Some(ControlRange { min, max }), None),
            Some(written::ControlRange { min, max }) => {
                let missing = match (min, max) {
                    (None, None) => "`min` and `max`",
                    (None, Some(_)) => "`min`",
                    _ => "`max`",
                };
                (None, Some(missing))
            }
        };
        Ok(Self {
            name,
            module,
            control,
            range,
            range_missing,
        })
    }
}

/// Fails the load of `definition`, the controls of a development or of a
/// document built as it is, when a control declares a range without both its
/// bounds. `whose` begins the message: the development (`Development 'voice':
/// `) or the document's file, or nothing when there is neither to name.
pub(crate) fn refuse_partial_ranges(
    whose: &str,
    definition: &Invention,
) -> Result<(), Box<dyn std::error::Error>> {
    match (definition.controls.iter()).find_map(|control| Some((control, control.range_missing?))) {
        None => Ok(()),
        Some((control, missing)) => Err(format!(
            "{whose}control '{}': range is missing {missing}: \
             a range needs both `min` and `max`",
            control.name
        )
        .into()),
    }
}
