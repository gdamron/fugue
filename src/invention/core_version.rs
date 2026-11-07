//! The minimum core a document declares it needs.
//!
//! An invention or development document may declare
//! `"minimum_core_version": "YYYY.M.P"`, the oldest Fugue core that reads all
//! of it. A core older than that refuses the document when it parses it,
//! rather than ignoring the fields it does not know and dropping them on the
//! next save. Saving writes the minimum the document's features need (see
//! [`Invention::declare_minimum_core`]).
//!
//! Cores released before the field existed ignore it like any unknown field,
//! so the refusal protects documents only from this core onward.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::format::Invention;

/// A Fugue core version, `YYYY.M.P`, ordered numerically per component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CoreVersion {
    year: u32,
    month: u32,
    patch: u32,
}

impl CoreVersion {
    /// The core that first reads a development control's `range`.
    pub const DEVELOPMENT_CONTROL_RANGES: CoreVersion = CoreVersion::new(2026, 11, 0);

    pub const fn new(year: u32, month: u32, patch: u32) -> Self {
        Self { year, month, patch }
    }

    /// The version of the core that is running: this crate's version.
    pub fn running() -> Self {
        let version = env!("CARGO_PKG_VERSION");
        // A pre-release or build suffix names the same release line.
        let release = version.split(['-', '+']).next().unwrap_or(version);
        release
            .parse()
            .expect("the crate version is a YYYY.M.P core version")
    }
}

impl fmt::Display for CoreVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.year, self.month, self.patch)
    }
}

/// A version that is not written as `YYYY.M.P`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreVersionParseError(String);

impl fmt::Display for CoreVersionParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` is not a core version; write it as YYYY.M.P, such as 2026.11.0",
            self.0
        )
    }
}

impl std::error::Error for CoreVersionParseError {}

impl FromStr for CoreVersion {
    type Err = CoreVersionParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || CoreVersionParseError(text.to_string());
        let mut parts = text.split('.').map(|part| {
            let digits = !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
            digits.then(|| part.parse::<u32>().ok()).flatten()
        });
        let (Some(Some(year)), Some(Some(month)), Some(Some(patch)), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid());
        };
        Ok(Self::new(year, month, patch))
    }
}

impl Serialize for CoreVersion {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for CoreVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// Reads a document's `minimum_core_version`, refusing one newer than the
/// running core. Refusing while parsing reaches every way a document comes
/// in: a file, a JSON string, a development loaded by path or reference, an
/// inline definition, and a daemon load or reload command.
pub(crate) fn deserialize_minimum<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<CoreVersion>, D::Error> {
    let Some(minimum) = Option::<CoreVersion>::deserialize(deserializer)? else {
        return Ok(None);
    };
    let running = CoreVersion::running();
    if minimum > running {
        return Err(serde::de::Error::custom(format!(
            "this document needs Fugue core {minimum} or newer, but the running core is \
             {running}; update Fugue to load it"
        )));
    }
    Ok(Some(minimum))
}

impl Invention {
    /// The oldest core that reads every feature this document uses, its
    /// inline development definitions included; `None` when it uses nothing
    /// that needs more than any core reading the field.
    ///
    /// A feature that needs a newer core adds its check here, with a new
    /// [`CoreVersion`] constant naming the first release that ships it. When
    /// that release is already tagged, the feature takes the next one and
    /// bumps the crate version with it, so a released core and a later build
    /// never share a version yet read different fields.
    pub fn features_minimum_core(&self) -> Option<CoreVersion> {
        let ranges = (self.controls.iter())
            .any(|control| control.range.is_some())
            .then_some(CoreVersion::DEVELOPMENT_CONTROL_RANGES);
        let definitions = (self.developments.iter())
            .filter_map(|spec| spec.definition.as_deref())
            .filter_map(Invention::features_minimum_core);
        ranges.into_iter().chain(definitions).max()
    }

    /// Raises `minimum_core_version`, here and in each inline development
    /// definition, to what the document's features need. A minimum the
    /// document already declares is never lowered.
    pub fn declare_minimum_core(&mut self) {
        for spec in &mut self.developments {
            if let Some(definition) = spec.definition.as_deref_mut() {
                definition.declare_minimum_core();
            }
        }
        // `None` orders below every version, so this keeps the higher one.
        self.minimum_core_version = self.minimum_core_version.max(self.features_minimum_core());
    }

    /// A copy with no declared minimum, here or in any inline definition:
    /// what the document builds, for comparing two versions of it.
    pub(crate) fn without_minimum_core(&self) -> Invention {
        let mut document = self.clone();
        document.clear_minimum_core();
        document
    }

    fn clear_minimum_core(&mut self) {
        self.minimum_core_version = None;
        for spec in &mut self.developments {
            if let Some(definition) = spec.definition.as_deref_mut() {
                definition.clear_minimum_core();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_order_numerically() {
        let parse = |text: &str| text.parse::<CoreVersion>().unwrap();
        assert!(parse("2026.10.0") > parse("2026.9.0"));
        assert!(parse("2027.1.0") > parse("2026.12.9"));
        assert_eq!(parse("2026.11.0").to_string(), "2026.11.0");
        for bad in [
            "",
            "soon",
            "2026.11",
            "2026.11.0.1",
            "v2026.11.0",
            "2026.+1.0",
            "2026..0",
        ] {
            assert!(bad.parse::<CoreVersion>().is_err(), "{bad:?} parses");
        }
    }

    #[test]
    fn every_feature_minimum_is_within_the_running_core() {
        // Saving writes these; a core that wrote one newer than itself
        // would refuse its own saves.
        assert!(CoreVersion::DEVELOPMENT_CONTROL_RANGES <= CoreVersion::running());
    }

    #[test]
    fn a_too_new_minimum_is_refused_naming_both_versions() {
        let json = r#"{"minimum_core_version":"9999.1.0","modules":[],"connections":[]}"#;
        let error = Invention::from_json(json).unwrap_err().to_string();
        assert!(error.contains("9999.1.0"), "{error}");
        assert!(
            error.contains(&CoreVersion::running().to_string()),
            "{error}"
        );
    }

    #[test]
    fn declaring_raises_but_never_lowers() {
        let ranged = r#"{"modules":[],"connections":[],"controls":[
            {"name":"rate","module":"lfo","control":"frequency","range":{"min":0.1,"max":5}}]}"#;
        let mut document = Invention::from_json(ranged).unwrap();
        document.declare_minimum_core();
        let ranges = Some(CoreVersion::DEVELOPMENT_CONTROL_RANGES);
        assert_eq!(document.minimum_core_version, ranges);

        document.minimum_core_version = Some(CoreVersion::new(2026, 1, 0));
        document.declare_minimum_core();
        assert_eq!(document.minimum_core_version, ranges);

        let mut plain = Invention::from_json(r#"{"modules":[],"connections":[]}"#).unwrap();
        plain.declare_minimum_core();
        assert_eq!(plain.minimum_core_version, None);
        plain.minimum_core_version = Some(CoreVersion::new(2026, 1, 0));
        plain.declare_minimum_core();
        assert_eq!(
            plain.minimum_core_version,
            Some(CoreVersion::new(2026, 1, 0))
        );
    }

    #[test]
    fn an_inline_definition_raises_its_parent() {
        let json = r#"{"modules":[],"connections":[],"developments":[{"name":"kit",
            "definition":{"modules":[],"connections":[],"controls":[
            {"name":"rate","module":"lfo","control":"frequency","range":{"min":0.1,"max":5}}]}}]}"#;
        let mut document = Invention::from_json(json).unwrap();
        document.declare_minimum_core();
        let ranges = Some(CoreVersion::DEVELOPMENT_CONTROL_RANGES);
        assert_eq!(document.minimum_core_version, ranges);
        let definition = document.developments[0].definition.as_deref().unwrap();
        assert_eq!(definition.minimum_core_version, ranges);
    }
}
