//! Parse errors that name what is wrong: the development, the control and
//! the bound a document leaves out.
//!
//! A derived `Deserialize` reports a range missing a bound as serde's bare
//! "missing field `min`", which names neither the development nor the
//! control. A development control reads its range here, so the error names
//! the control and the missing bound, and a development declared in a
//! document reads its inline definition here, so whatever fails inside it
//! names the development.
//!
//! Both refuse a field written twice, as serde's derives do. An inline
//! definition is read straight from the source when the development's name
//! comes first; when the definition comes first (a document built from a
//! sorted map writes it so) it is held until the name is read, in a value
//! that notes a key written twice anywhere in it, which is then refused.

use std::fmt;

use serde::de::{DeserializeSeed, Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use super::{ControlRange, DevelopmentControl, DevelopmentSpec, Invention};
use crate::pkg::content::ContentRef;

/// A range as written, with either bound possibly absent.
#[derive(Deserialize)]
struct WrittenRange {
    min: Option<f64>,
    max: Option<f64>,
}

impl WrittenRange {
    /// The two-sided range, or which bounds it leaves out.
    fn complete(self) -> Result<ControlRange, String> {
        match (self.min, self.max) {
            (Some(min), Some(max)) => Ok(ControlRange { min, max }),
            (min, max) => {
                let missing: Vec<&str> = [(min, "`min`"), (max, "`max`")]
                    .into_iter()
                    .filter(|(bound, _)| bound.is_none())
                    .map(|(_, name)| name)
                    .collect();
                Err(format!(
                    "range is missing {}: a range needs both `min` and `max`",
                    missing.join(" and ")
                ))
            }
        }
    }
}

impl<'de> Deserialize<'de> for DevelopmentControl {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Fields {
            name: String,
            module: String,
            control: String,
            range: Option<WrittenRange>,
        }
        let Fields {
            name,
            module,
            control,
            range,
        } = Fields::deserialize(deserializer)?;
        let range = range
            .map(WrittenRange::complete)
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

/// `message` without the " at line N column M" a serde_json error ends
/// with, for an error that is wrapped and given its position again.
fn without_position(message: &str) -> &str {
    let numbered = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    match message.rsplit_once(" at line ") {
        Some((head, position))
            if position
                .split_once(" column ")
                .is_some_and(|(line, column)| numbered(line) && numbered(column)) =>
        {
            head
        }
        _ => message,
    }
}

/// `error`, from reading the definition of development `name`, naming it.
fn named<E: serde::de::Error>(name: &str, error: impl fmt::Display) -> E {
    E::custom(format!(
        "Development '{name}': {}",
        without_position(&error.to_string())
    ))
}

/// Reads an inline definition from the source, once the development's name
/// is known, naming it in what fails inside.
struct Definition<'a> {
    development: &'a str,
}

impl<'de> DeserializeSeed<'de> for Definition<'_> {
    type Value = Option<Box<Invention>>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        Option::<Box<Invention>>::deserialize(deserializer)
            .map_err(|why| named(self.development, why))
    }
}

/// A JSON value that holds a definition read before its development's name,
/// and the first key written twice in any object in it, which is refused
/// once the name is known.
struct Held {
    value: serde_json::Value,
    duplicate: Option<String>,
}

impl<'de> Deserialize<'de> for Held {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Reading;

        impl<'de> Visitor<'de> for Reading {
            type Value = Held;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("any valid JSON value")
            }

            fn visit_bool<E>(self, value: bool) -> Result<Held, E> {
                Ok(Held::of(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Held, E> {
                Ok(Held::of(value))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Held, E> {
                Ok(Held::of(value))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Held, E> {
                Ok(Held::of(value))
            }

            fn visit_str<E>(self, value: &str) -> Result<Held, E> {
                Ok(Held::of(value))
            }

            fn visit_none<E>(self) -> Result<Held, E> {
                Ok(Held::of(serde_json::Value::Null))
            }

            fn visit_unit<E>(self) -> Result<Held, E> {
                Ok(Held::of(serde_json::Value::Null))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Held, A::Error> {
                let mut items = Vec::new();
                let mut duplicate = None;
                while let Some(item) = seq.next_element::<Held>()? {
                    duplicate = duplicate.or(item.duplicate);
                    items.push(item.value);
                }
                Ok(Held {
                    value: items.into(),
                    duplicate,
                })
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Held, A::Error> {
                let mut fields = serde_json::Map::new();
                let mut duplicate = None;
                while let Some(key) = map.next_key::<String>()? {
                    if fields.contains_key(&key) {
                        // The value is still read, so the source stays in
                        // step and the name can be read after it.
                        map.next_value::<IgnoredAny>()?;
                        duplicate = duplicate.or(Some(key));
                        continue;
                    }
                    let item: Held = map.next_value()?;
                    duplicate = duplicate.or(item.duplicate);
                    fields.insert(key, item.value);
                }
                Ok(Held {
                    value: fields.into(),
                    duplicate,
                })
            }
        }

        deserializer.deserialize_any(Reading)
    }
}

impl Held {
    fn of(value: impl Into<serde_json::Value>) -> Self {
        Self {
            value: value.into(),
            duplicate: None,
        }
    }
}

/// A definition as read: from the source, or held for want of a name.
enum Read {
    Definition(Option<Box<Invention>>),
    Held(Option<Held>),
}

#[derive(Deserialize)]
#[serde(field_identifier)]
enum SpecField {
    #[serde(rename = "name")]
    Name,
    #[serde(rename = "path")]
    Path,
    #[serde(rename = "ref")]
    Reference,
    #[serde(rename = "definition")]
    Definition,
    #[serde(other)]
    Other,
}

struct SpecVisitor;

impl<'de> Visitor<'de> for SpecVisitor {
    type Value = DevelopmentSpec;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("struct DevelopmentSpec")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<DevelopmentSpec, A::Error> {
        let mut name: Option<String> = None;
        let mut path: Option<Option<String>> = None;
        let mut reference: Option<Option<ContentRef>> = None;
        let mut definition: Option<Read> = None;
        while let Some(field) = map.next_key()? {
            match field {
                SpecField::Name if name.is_some() => {
                    return Err(A::Error::duplicate_field("name"));
                }
                SpecField::Name => name = Some(map.next_value()?),
                SpecField::Path if path.is_some() => {
                    return Err(A::Error::duplicate_field("path"));
                }
                SpecField::Path => path = Some(map.next_value()?),
                SpecField::Reference if reference.is_some() => {
                    return Err(A::Error::duplicate_field("ref"));
                }
                SpecField::Reference => reference = Some(map.next_value()?),
                SpecField::Definition if definition.is_some() => {
                    return Err(A::Error::duplicate_field("definition"));
                }
                SpecField::Definition => {
                    definition = Some(match name.as_deref() {
                        Some(development) => {
                            Read::Definition(map.next_value_seed(Definition { development })?)
                        }
                        None => Read::Held(map.next_value()?),
                    });
                }
                SpecField::Other => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        let name = name.ok_or_else(|| A::Error::missing_field("name"))?;
        let definition = match definition {
            None | Some(Read::Definition(None) | Read::Held(None)) => None,
            Some(Read::Definition(read)) => read,
            Some(Read::Held(Some(Held {
                duplicate: Some(key),
                ..
            }))) => {
                return Err(named(&name, format!("duplicate field `{key}`")));
            }
            Some(Read::Held(Some(Held { value, .. }))) => Some(Box::new(
                Invention::deserialize(value).map_err(|why| named::<A::Error>(&name, why))?,
            )),
        };
        Ok(DevelopmentSpec {
            name,
            path: path.flatten(),
            reference: reference.flatten(),
            definition,
        })
    }
}

impl<'de> Deserialize<'de> for DevelopmentSpec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["name", "path", "ref", "definition"];
        deserializer.deserialize_struct("DevelopmentSpec", FIELDS, SpecVisitor)
    }
}
