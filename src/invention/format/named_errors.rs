//! Parse errors that name what is wrong: the development, the control and
//! the bound a document leaves out.
//!
//! A derived `Deserialize` reports a range missing a bound as serde's bare
//! "missing field `min`", which names neither the development nor the
//! control. A development control reads its range through here, so the
//! error names the control and the missing bound, and a development
//! declared in a document reads its inline definition through here, so
//! whatever fails inside it names the development.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use super::{ControlRange, DevelopmentControl, DevelopmentSpec, Invention};
use crate::pkg::content::ContentRef;

/// Reads `range`, a control's two-sided range, or says which bounds it
/// leaves out.
fn read_range(range: Value) -> Result<ControlRange, String> {
    if let Value::Object(fields) = &range {
        let missing: Vec<String> = ["min", "max"]
            .into_iter()
            .filter(|bound| !fields.contains_key(*bound))
            .map(|bound| format!("`{bound}`"))
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "range is missing {}: a range needs both `min` and `max`",
                missing.join(" and ")
            ));
        }
    }
    serde_json::from_value(range).map_err(|err| format!("range: {err}"))
}

impl<'de> Deserialize<'de> for DevelopmentControl {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Fields {
            name: String,
            module: String,
            control: String,
            #[serde(default)]
            range: Option<Value>,
        }
        let Fields {
            name,
            module,
            control,
            range,
        } = Fields::deserialize(deserializer)?;
        let range = range
            .map(read_range)
            .transpose()
            .map_err(|why| D::Error::custom(format!("control '{name}': {why}")))?;
        Ok(Self {
            name,
            module,
            control,
            range,
        })
    }
}

impl<'de> Deserialize<'de> for DevelopmentSpec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Fields {
            name: String,
            #[serde(default)]
            path: Option<String>,
            #[serde(default, rename = "ref")]
            reference: Option<ContentRef>,
            #[serde(default)]
            definition: Option<Value>,
        }
        let Fields {
            name,
            path,
            reference,
            definition,
        } = Fields::deserialize(deserializer)?;
        let definition = definition
            .map(|definition| Invention::deserialize(definition).map(Box::new))
            .transpose()
            .map_err(|why| D::Error::custom(format!("Development '{name}': {why}")))?;
        Ok(Self {
            name,
            path,
            reference,
            definition,
        })
    }
}
