//! Chooses the output device the cpal driver plays to.
//!
//! By default that is the host's default output. [`OUTPUT_DEVICE_ENV`] names
//! another device by its exact name, e.g. a loopback device such as
//! `BlackHole 16ch` that an agent can record without playing anything
//! through the listener's speakers. A name that matches no device is an
//! error listing the devices that exist; it never falls back to the default
//! output, so audio never goes somewhere it was not sent.

use cpal::traits::{DeviceTrait, HostTrait};

#[cfg(test)]
pub(super) use fake::FakeHost;

/// Names the output device to play to instead of the system default, e.g.
/// `FUGUE_OUTPUT_DEVICE="BlackHole 16ch"`. The name must match a device's
/// name exactly. Unset or empty means the system default output.
pub const OUTPUT_DEVICE_ENV: &str = "FUGUE_OUTPUT_DEVICE";

/// The device name [`OUTPUT_DEVICE_ENV`] asks for, or `None` when it is
/// unset or empty.
pub(super) fn requested_output_device() -> Option<String> {
    requested_name(std::env::var_os(OUTPUT_DEVICE_ENV).as_deref())
}

/// Interprets a raw [`OUTPUT_DEVICE_ENV`] value. A value that is not valid
/// Unicode names no device that could exist, so it is kept (lossily) and
/// refused by the match rather than treated as unset.
fn requested_name(raw: Option<&std::ffi::OsStr>) -> Option<String> {
    let raw = raw?;
    if raw.is_empty() {
        return None;
    }
    Some(raw.to_string_lossy().into_owned())
}

/// Every output device a host lists, each with its name when the name could
/// be read.
pub(super) type NamedOutputs<D> = Result<Vec<(D, Option<String>)>, Box<dyn std::error::Error>>;

/// The output devices an audio host offers. cpal's host implements it; tests
/// stand in fake hosts, so the choice between the default output and a named
/// one is checked without hardware.
pub(super) trait OutputHost {
    type Device;

    /// The host's default output device, if it has one.
    fn default_output(&self) -> Option<Self::Device>;

    /// Every output device, each with its name when the name could be read.
    fn outputs(&self) -> NamedOutputs<Self::Device>;
}

impl OutputHost for cpal::Host {
    type Device = cpal::Device;

    fn default_output(&self) -> Option<cpal::Device> {
        self.default_output_device()
    }

    fn outputs(&self) -> NamedOutputs<cpal::Device> {
        Ok(self
            .output_devices()?
            .map(|device| {
                let name = device
                    .description()
                    .ok()
                    .map(|description| description.name().to_string());
                (device, name)
            })
            .collect())
    }
}

/// The output device to play to: the one named `wanted`, or the host's
/// default output when `wanted` is `None`.
///
/// # Errors
///
/// Returns an error if there is no default output, the devices cannot be
/// listed, or no device is named `wanted`. With a name, the default output is
/// never consulted, so a missing named device is never replaced by it.
pub(super) fn output_device<H: OutputHost>(
    host: &H,
    wanted: Option<&str>,
) -> Result<H::Device, Box<dyn std::error::Error>> {
    let Some(wanted) = wanted else {
        return Ok(host.default_output().ok_or("No output device available")?);
    };
    Ok(select_named(wanted, host.outputs()?)?)
}

/// Picks the first device whose name is exactly `wanted`. Devices whose name
/// could not be read are skipped. When none matches, the error lists every
/// name that was seen.
fn select_named<D>(
    wanted: &str,
    devices: impl IntoIterator<Item = (D, Option<String>)>,
) -> Result<D, MissingOutputDevice> {
    let mut available = Vec::new();
    for (device, name) in devices {
        let Some(name) = name else {
            continue;
        };
        if name == wanted {
            return Ok(device);
        }
        available.push(name);
    }
    Err(MissingOutputDevice {
        wanted: wanted.to_string(),
        available,
    })
}

/// No output device has the name [`OUTPUT_DEVICE_ENV`] gave.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MissingOutputDevice {
    wanted: String,
    available: Vec<String>,
}

impl std::fmt::Display for MissingOutputDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{OUTPUT_DEVICE_ENV} names \"{}\", but no output device has that name; available: ",
            self.wanted
        )?;
        if self.available.is_empty() {
            return f.write_str("none");
        }
        let quoted: Vec<String> = self
            .available
            .iter()
            .map(|name| format!("\"{name}\""))
            .collect();
        f.write_str(&quoted.join(", "))
    }
}

impl std::error::Error for MissingOutputDevice {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn devices(names: &[Option<&str>]) -> Vec<(usize, Option<String>)> {
        names
            .iter()
            .enumerate()
            .map(|(index, name)| (index, name.map(str::to_string)))
            .collect()
    }

    #[test]
    fn an_unset_or_empty_variable_asks_for_the_default_output() {
        assert_eq!(requested_name(None), None);
        assert_eq!(requested_name(Some(OsStr::new(""))), None);
        assert_eq!(
            requested_name(Some(OsStr::new("BlackHole 16ch"))).as_deref(),
            Some("BlackHole 16ch")
        );
    }

    #[test]
    fn whitespace_is_part_of_the_name_not_unset() {
        assert_eq!(requested_name(Some(OsStr::new(" "))).as_deref(), Some(" "));
        let list = devices(&[Some("BlackHole 16ch")]);
        assert!(select_named(" BlackHole 16ch", list).is_err());
    }

    #[test]
    fn the_device_with_the_exact_name_is_chosen() {
        let list = devices(&[Some("MacBook Speakers"), Some("BlackHole 16ch")]);
        assert_eq!(select_named("BlackHole 16ch", list), Ok(1));
    }

    #[test]
    fn the_first_of_several_devices_sharing_a_name_is_chosen() {
        let list = devices(&[Some("Speakers"), Some("Loop"), Some("Loop")]);
        assert_eq!(select_named("Loop", list), Ok(1));
    }

    #[test]
    fn a_name_matches_only_exactly() {
        for near_miss in ["BlackHole", "blackhole 16ch", "BlackHole 16ch "] {
            let list = devices(&[Some("BlackHole 16ch")]);
            assert!(
                select_named(near_miss, list).is_err(),
                "{near_miss:?} should not match"
            );
        }
    }

    #[test]
    fn an_unknown_name_is_refused_with_the_names_available() {
        let list = devices(&[Some("BlackHole 16ch"), None, Some("Speakers")]);
        let error = select_named("Studio", list).unwrap_err();
        assert_eq!(
            error,
            MissingOutputDevice {
                wanted: "Studio".into(),
                available: vec!["BlackHole 16ch".into(), "Speakers".into()],
            }
        );
        assert_eq!(
            error.to_string(),
            "FUGUE_OUTPUT_DEVICE names \"Studio\", but no output device has that name; \
             available: \"BlackHole 16ch\", \"Speakers\""
        );
    }

    #[test]
    fn an_unknown_name_with_no_devices_says_none_are_available() {
        let error = select_named("Studio", devices(&[None])).unwrap_err();
        assert!(error.to_string().ends_with("available: none"), "{error}");
    }

    #[test]
    fn without_a_name_the_default_output_is_used() {
        let host = FakeHost::new("Speakers", &["Speakers", "BlackHole 16ch"]);
        assert_eq!(output_device(&host, None).unwrap(), "Speakers");
    }

    #[test]
    fn a_name_never_consults_the_default_output() {
        let host = FakeHost::new("Speakers", &["Speakers", "BlackHole 16ch"]);
        assert_eq!(
            output_device(&host, Some("BlackHole 16ch")).unwrap(),
            "BlackHole 16ch"
        );
        assert!(output_device(&host, Some("Studio")).is_err());
        assert_eq!(host.default_lookups(), 0);
    }

    #[test]
    fn a_device_list_that_cannot_be_read_is_an_error_not_the_default() {
        let host = FakeHost::new("Speakers", &["Speakers"]);
        host.set_listing_fails(true);
        assert!(output_device(&host, Some("Speakers")).is_err());
        assert_eq!(host.default_lookups(), 0);
    }

    /// Lists the real host's devices but opens none. Run explicitly with
    /// `cargo test -- --ignored a_bogus_name_is_refused_by_the_real_host`.
    #[test]
    #[ignore = "needs an audio host"]
    fn a_bogus_name_is_refused_by_the_real_host() {
        let bogus = "fugue: no device has this name";
        let Err(error) = output_device(&cpal::default_host(), Some(bogus)) else {
            panic!("a bogus name must be refused, never replaced by the default");
        };
        let message = error.to_string();
        assert!(message.contains(bogus), "{message}");
        assert!(message.contains("available: "), "{message}");
        println!("{message}");
    }
}

#[cfg(test)]
mod fake {
    use super::{NamedOutputs, OutputHost};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// A host whose devices are names. It counts default-output lookups so
    /// tests can show a named request never reaches the default.
    pub(in crate::modules::dac) struct FakeHost {
        default: &'static str,
        devices: Mutex<Vec<&'static str>>,
        listing_fails: AtomicBool,
        default_lookups: AtomicUsize,
    }

    impl FakeHost {
        pub(in crate::modules::dac) fn new(
            default: &'static str,
            devices: &[&'static str],
        ) -> Self {
            Self {
                default,
                devices: Mutex::new(devices.to_vec()),
                listing_fails: AtomicBool::new(false),
                default_lookups: AtomicUsize::new(0),
            }
        }

        pub(in crate::modules::dac) fn set_devices(&self, devices: &[&'static str]) {
            *self.devices.lock().unwrap() = devices.to_vec();
        }

        pub(in crate::modules::dac) fn set_listing_fails(&self, fails: bool) {
            self.listing_fails.store(fails, Ordering::SeqCst);
        }

        pub(in crate::modules::dac) fn default_lookups(&self) -> usize {
            self.default_lookups.load(Ordering::SeqCst)
        }
    }

    impl OutputHost for FakeHost {
        type Device = &'static str;

        fn default_output(&self) -> Option<&'static str> {
            self.default_lookups.fetch_add(1, Ordering::SeqCst);
            Some(self.default)
        }

        fn outputs(&self) -> NamedOutputs<&'static str> {
            if self.listing_fails.load(Ordering::SeqCst) {
                return Err("the device list could not be read".into());
            }
            let devices = self.devices.lock().unwrap();
            Ok(devices
                .iter()
                .map(|name| (*name, Some(name.to_string())))
                .collect())
        }
    }
}
