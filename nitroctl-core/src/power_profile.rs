//! Power-profile control via `power-profiles-daemon`, per docs/architecture.md
//! and FR-005. `PowerProfilesBackend` is the testing seam (mirrors
//! `SysfsReader`/`CommandRunner`): all D-Bus IO lives behind it, so the
//! validation/state-mapping logic in `PowerProfilesDaemon` is unit-testable
//! without a live D-Bus connection.

use crate::capability::CapabilityState;
use crate::sysfs::SysfsReader;

/// One entry of PPD's `Profiles` D-Bus property (verified shape via
/// `busctl introspect`, docs/hardware.md M3): each dict has a `Profile`
/// name and, only for a placeholder-backed profile, `PlatformDriver` ==
/// `"placeholder"`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileInfo {
    pub name: String,
    pub is_placeholder: bool,
    /// `true` when this exact name is known in advance to fail if written,
    /// on hardware backed by a real ACPI `platform_profile` driver -- not a
    /// transient error, a documented permanent firmware/EC limitation (see
    /// `KNOWN_UNSUPPORTED_PROFILE`). Distinct from `is_placeholder`: a
    /// placeholder profile *succeeds* as a no-op; this one *fails* outright.
    pub known_unsupported: bool,
}

/// The one ACPI `platform_profile` value this project has confirmed (M5,
/// docs/hardware.md's `predator_v4=1` experiment) the EC firmware rejects
/// with `-EIO` on this hardware and three sibling Nitro/Predator models --
/// a permanent hardware ceiling, not a transient bug. Shared so every
/// backend keys off the same documented name instead of separate literals.
const KNOWN_UNSUPPORTED_PROFILE: &str = "performance";

#[derive(Debug, Clone, PartialEq)]
pub enum BackendError {
    /// `power-profiles-daemon` isn't running / its bus name has no owner.
    Unavailable,
    /// The D-Bus call was rejected for lack of authorization.
    Denied,
    /// Any other IO/protocol failure; the message names the interface and
    /// underlying error per SAFE-004.
    Other(String),
}

/// The IO seam: talks to `power-profiles-daemon` over D-Bus. See
/// `ZbusPowerProfilesBackend` for the real implementation.
pub trait PowerProfilesBackend: Send + Sync {
    fn profiles(&self) -> Result<Vec<ProfileInfo>, BackendError>;
    fn active_profile_name(&self) -> Result<String, BackendError>;
    fn set_active_profile(&self, name: &str) -> Result<(), BackendError>;
}

impl<T: PowerProfilesBackend + ?Sized> PowerProfilesBackend for std::sync::Arc<T> {
    fn profiles(&self) -> Result<Vec<ProfileInfo>, BackendError> {
        (**self).profiles()
    }
    fn active_profile_name(&self) -> Result<String, BackendError> {
        (**self).active_profile_name()
    }
    fn set_active_profile(&self, name: &str) -> Result<(), BackendError> {
        (**self).set_active_profile(name)
    }
}

/// A profile name plus whether it's known to actually affect hardware.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileStatus {
    pub name: String,
    /// `false` when this profile is backed by PPD's "placeholder" driver
    /// (docs/hardware.md) — real, switchable, but a no-op on this hardware.
    pub hardware_backed: bool,
}

/// Per cli.md/SAFE-003/SAFE-004: rejects invalid input before writing, and
/// a failed write is reported rather than assumed to have succeeded.
#[derive(Debug, Clone, PartialEq)]
pub enum ProfileError {
    InvalidProfile {
        requested: String,
        valid: Vec<String>,
    },
    /// The name is a real, listed choice, but writing it is known in
    /// advance to fail -- a documented permanent firmware/EC limitation
    /// (see `KNOWN_UNSUPPORTED_PROFILE`), not a NitroControl bug. Rejected
    /// before any write is attempted, same as `InvalidProfile`.
    KnownUnsupportedProfile {
        requested: String,
    },
    BackendUnavailable,
    BackendDenied,
    BackendFailed(String),
}

/// `Send + Sync` for the same reason as `SensorProvider` — a long-lived
/// instance can be shared with a background polling thread instead of
/// reconnecting to D-Bus every tick.
pub trait PowerProfileProvider: Send + Sync {
    fn list_profiles(&self) -> CapabilityState<Vec<String>>;
    /// Same as `list_profiles()`, but with each entry's full detail (e.g.
    /// `known_unsupported`, issue #25) instead of just its name -- for
    /// callers (the GUI) that need to gray out a specific known-bad choice
    /// rather than treat the whole list as all-or-nothing.
    fn list_profile_details(&self) -> CapabilityState<Vec<ProfileInfo>>;
    fn current_profile(&self) -> CapabilityState<ProfileStatus>;
    fn set_profile(&self, profile: &str) -> Result<(), ProfileError>;
}

fn map_backend_error<T>(err: BackendError) -> CapabilityState<T> {
    match err {
        BackendError::Unavailable => CapabilityState::Unsupported,
        BackendError::Denied => CapabilityState::RequiresPrivilege,
        BackendError::Other(_) => CapabilityState::Unknown,
    }
}

pub struct PowerProfilesDaemon<B: PowerProfilesBackend> {
    backend: B,
}

impl<B: PowerProfilesBackend> PowerProfilesDaemon<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

impl<B: PowerProfilesBackend> PowerProfileProvider for PowerProfilesDaemon<B> {
    fn list_profiles(&self) -> CapabilityState<Vec<String>> {
        match self.list_profile_details() {
            CapabilityState::Supported(v) => {
                CapabilityState::Supported(v.into_iter().map(|p| p.name).collect())
            }
            CapabilityState::HardwareDependent(v) => {
                CapabilityState::HardwareDependent(v.into_iter().map(|p| p.name).collect())
            }
            CapabilityState::Unsupported => CapabilityState::Unsupported,
            CapabilityState::Unknown => CapabilityState::Unknown,
            CapabilityState::RequiresPrivilege => CapabilityState::RequiresPrivilege,
        }
    }

    fn list_profile_details(&self) -> CapabilityState<Vec<ProfileInfo>> {
        match self.backend.profiles() {
            Ok(profiles) => CapabilityState::Supported(profiles),
            Err(e) => map_backend_error(e),
        }
    }

    fn current_profile(&self) -> CapabilityState<ProfileStatus> {
        let profiles = match self.backend.profiles() {
            Ok(p) => p,
            Err(e) => return map_backend_error(e),
        };
        let name = match self.backend.active_profile_name() {
            Ok(n) => n,
            Err(e) => return map_backend_error(e),
        };
        let is_placeholder = profiles
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.is_placeholder)
            .unwrap_or(false);
        let status = ProfileStatus {
            name,
            hardware_backed: !is_placeholder,
        };
        if is_placeholder {
            CapabilityState::HardwareDependent(status)
        } else {
            CapabilityState::Supported(status)
        }
    }

    fn set_profile(&self, profile: &str) -> Result<(), ProfileError> {
        let profiles = self.backend.profiles().map_err(|e| match e {
            BackendError::Unavailable => ProfileError::BackendUnavailable,
            BackendError::Denied => ProfileError::BackendDenied,
            BackendError::Other(msg) => ProfileError::BackendFailed(msg),
        })?;
        let matched = profiles.iter().find(|p| p.name == profile).cloned();
        let valid: Vec<String> = profiles.into_iter().map(|p| p.name).collect();
        let Some(info) = matched else {
            return Err(ProfileError::InvalidProfile {
                requested: profile.to_string(),
                valid,
            });
        };
        if info.known_unsupported {
            return Err(ProfileError::KnownUnsupportedProfile {
                requested: profile.to_string(),
            });
        }

        self.backend
            .set_active_profile(profile)
            .map_err(|e| match e {
                BackendError::Unavailable => ProfileError::BackendUnavailable,
                BackendError::Denied => ProfileError::BackendDenied,
                BackendError::Other(msg) => ProfileError::BackendFailed(msg),
            })
    }
}

/// Real `power-profiles-daemon` client over the system D-Bus, per the
/// contract verified live in docs/hardware.md (M3): bus name
/// `org.freedesktop.UPower.PowerProfiles` (falling back to the legacy
/// `net.hadess.PowerProfiles` name for older distro PPD versions), object
/// path `/org/freedesktop/UPower/PowerProfiles`, `Profiles`/`ActiveProfile`
/// properties. Uses `zbus::blocking` — no async runtime needed for a
/// one-shot CLI call.
pub struct ZbusPowerProfilesBackend {
    connection: zbus::blocking::Connection,
    bus_name: String,
}

const PRIMARY_BUS_NAME: &str = "org.freedesktop.UPower.PowerProfiles";
const LEGACY_BUS_NAME: &str = "net.hadess.PowerProfiles";
const OBJECT_PATH: &str = "/org/freedesktop/UPower/PowerProfiles";
const INTERFACE: &str = "org.freedesktop.UPower.PowerProfiles";

impl ZbusPowerProfilesBackend {
    /// Connects to the system bus and probes for whichever bus name this
    /// system's `power-profiles-daemon` actually registers, preferring the
    /// current name over the legacy one.
    ///
    /// If neither candidate answers, the real cause is preserved rather than
    /// collapsed to a blanket `Unavailable`: a candidate whose name simply
    /// has no owner (confirmed live: `zbus`'s error `Display` for that case
    /// contains `"ServiceUnknown"`) genuinely means "not installed/running",
    /// but `Denied`/`Other` mean something is actually wrong (bad D-Bus
    /// policy, protocol error, etc.) and must not be reported as if the
    /// service were merely absent.
    pub fn connect() -> Result<Self, BackendError> {
        let connection =
            zbus::blocking::Connection::system().map_err(|e| BackendError::Other(e.to_string()))?;

        let mut most_specific_error: Option<BackendError> = None;
        for candidate in [PRIMARY_BUS_NAME, LEGACY_BUS_NAME] {
            match Self::probe(&connection, candidate) {
                Ok(()) => {
                    return Ok(Self {
                        connection,
                        bus_name: candidate.to_string(),
                    })
                }
                Err(BackendError::Unavailable) => {} // keep trying the next candidate
                Err(other) if most_specific_error.is_none() => most_specific_error = Some(other),
                Err(_) => {}
            }
        }
        Err(most_specific_error.unwrap_or(BackendError::Unavailable))
    }

    fn probe(connection: &zbus::blocking::Connection, bus_name: &str) -> Result<(), BackendError> {
        Self::proxy_for(connection, bus_name)
            .map_err(map_zbus_error)?
            .get_property::<String>("Version")
            .map_err(map_zbus_error)?;
        Ok(())
    }

    fn proxy_for(
        connection: &zbus::blocking::Connection,
        bus_name: &str,
    ) -> zbus::Result<zbus::blocking::Proxy<'static>> {
        zbus::blocking::Proxy::new(connection, bus_name.to_string(), OBJECT_PATH, INTERFACE)
    }

    fn proxy(&self) -> Result<zbus::blocking::Proxy<'static>, BackendError> {
        Self::proxy_for(&self.connection, &self.bus_name).map_err(map_zbus_error)
    }
}

fn value_as_string(value: &zbus::zvariant::OwnedValue) -> Option<String> {
    let owned = value.try_clone().ok()?;
    zbus::zvariant::Str::try_from(owned)
        .ok()
        .map(|s| s.to_string())
}

fn map_zbus_error(err: zbus::Error) -> BackendError {
    let message = err.to_string();
    if message.contains("AccessDenied")
        || message.contains("AuthFailed")
        || message.contains("NotAuthorized")
        || message.contains("InteractiveAuthorizationRequired")
    {
        BackendError::Denied
    } else if message.contains("ServiceUnknown") || message.contains("NameHasNoOwner") {
        BackendError::Unavailable
    } else {
        BackendError::Other(message)
    }
}

/// Parses one entry of PPD's `Profiles` D-Bus property into a `ProfileInfo`.
/// A free function (not a method) so it's unit-testable without a live
/// D-Bus connection -- see the `zbus_profile_parsing` tests.
fn profile_info_from_dict(
    dict: &std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
) -> Result<ProfileInfo, BackendError> {
    let name = dict
        .get("Profile")
        .and_then(value_as_string)
        .ok_or_else(|| BackendError::Other("Profiles entry missing 'Profile' name".to_string()))?;
    let platform_driver = dict.get("PlatformDriver").and_then(value_as_string);
    let is_placeholder = platform_driver.as_deref() == Some("placeholder");
    // Only known-unsupported when PPD has adopted the real ACPI
    // platform_profile driver for this name -- see KNOWN_UNSUPPORTED_PROFILE.
    // On this machine's default state (docs/hardware.md line 62) `performance`
    // is CpuDriver `amd_pstate`-backed instead and genuinely works.
    let known_unsupported =
        name == KNOWN_UNSUPPORTED_PROFILE && platform_driver.as_deref() == Some("platform_profile");
    Ok(ProfileInfo {
        name,
        is_placeholder,
        known_unsupported,
    })
}

impl PowerProfilesBackend for ZbusPowerProfilesBackend {
    fn profiles(&self) -> Result<Vec<ProfileInfo>, BackendError> {
        let proxy = self.proxy()?;
        let raw: Vec<std::collections::HashMap<String, zbus::zvariant::OwnedValue>> =
            proxy.get_property("Profiles").map_err(map_zbus_error)?;

        raw.iter().map(profile_info_from_dict).collect()
    }

    fn active_profile_name(&self) -> Result<String, BackendError> {
        let proxy = self.proxy()?;
        proxy
            .get_property::<String>("ActiveProfile")
            .map_err(map_zbus_error)
    }

    fn set_active_profile(&self, name: &str) -> Result<(), BackendError> {
        let proxy = self.proxy()?;
        proxy
            .set_property("ActiveProfile", name)
            .map_err(|e| map_zbus_error(zbus::Error::from(e)))
    }
}

/// Fallback backend for when connecting to `power-profiles-daemon` itself
/// failed. Every call reports the *same* error `connect()` actually
/// returned — never hardcoded to `Unavailable` — so a real `Denied`/`Other`
/// connection failure still reaches `CapabilityState::RequiresPrivilege`/
/// `Unknown` (and `ProfileError::BackendDenied`/`BackendFailed` for
/// `set_profile`) instead of being misreported as "service not installed".
/// This means `main.rs` never has to special-case "no provider at all"
/// separately from "provider reported an error" — it always has a working
/// `PowerProfileProvider` to call.
pub struct FailedBackend(pub BackendError);

impl PowerProfilesBackend for FailedBackend {
    fn profiles(&self) -> Result<Vec<ProfileInfo>, BackendError> {
        Err(self.0.clone())
    }
    fn active_profile_name(&self) -> Result<String, BackendError> {
        Err(self.0.clone())
    }
    fn set_active_profile(&self, _name: &str) -> Result<(), BackendError> {
        Err(self.0.clone())
    }
}

/// Acer-firmware power-profile backend (M5, FR-007) — reads/writes
/// `/sys/firmware/acpi/platform_profile` directly via `SysfsReader`,
/// instead of `power-profiles-daemon`'s D-Bus interface. See
/// docs/architecture.md's "Acer-firmware power profile (M5, FR-007)"
/// section for why this is a second backend rather than routed through
/// PPD: PPD's 3-profile set would collapse the 5 real ACPI values
/// docs/hardware.md's M5 experiment measured distinct fan behavior for.
///
/// Both files are only present when `acer_wmi` is loaded with
/// `predator_v4=1` — absent by default, which reads as `Unavailable`
/// (`CapabilityState::Unsupported` once through `PowerProfilesDaemon`),
/// exactly like any other not-installed backend. NitroControl never loads
/// that module parameter itself (SAFE-001/SAFE-002).
pub struct AcerPlatformProfileBackend<R: SysfsReader> {
    sysfs: R,
}

const PLATFORM_PROFILE_PATH: &str = "/sys/firmware/acpi/platform_profile";
const PLATFORM_PROFILE_CHOICES_PATH: &str = "/sys/firmware/acpi/platform_profile_choices";

impl<R: SysfsReader> AcerPlatformProfileBackend<R> {
    pub fn new(sysfs: R) -> Self {
        Self { sysfs }
    }
}

/// Classifies a sysfs IO error the same way for both reads and writes:
/// the file/node not existing means the backend isn't active (absence is
/// the *normal*, default state here — see the struct doc comment), a
/// permission error means privilege is genuinely required, anything else
/// (e.g. the real `-EIO` docs/hardware.md's M5 experiment found writing
/// `performance`) is reported verbatim per SAFE-004 rather than guessed at.
fn map_sysfs_error(err: std::io::Error) -> BackendError {
    match err.kind() {
        std::io::ErrorKind::NotFound => BackendError::Unavailable,
        std::io::ErrorKind::PermissionDenied => BackendError::Denied,
        _ => BackendError::Other(err.to_string()),
    }
}

impl<R: SysfsReader> PowerProfilesBackend for AcerPlatformProfileBackend<R> {
    fn profiles(&self) -> Result<Vec<ProfileInfo>, BackendError> {
        let raw = self
            .sysfs
            .read_to_string(std::path::Path::new(PLATFORM_PROFILE_CHOICES_PATH))
            .map_err(map_sysfs_error)?;
        Ok(raw
            .split_whitespace()
            .map(|name| ProfileInfo {
                name: name.to_string(),
                // No placeholder concept applies to this backend — every
                // choice ACPI advertises is a real value NitroControl can
                // attempt to write; whether it's *accepted* (hardware_backed
                // in ProfileStatus) is only known once set_active_profile()
                // is actually tried (docs/hardware.md: 4 of 5 do, one EIOs).
                is_placeholder: false,
                // This backend only exists when the real ACPI platform_profile
                // node is present, so it's always the "real ACPI backing"
                // case -- no driver-string check needed, unlike the PPD
                // backend. See KNOWN_UNSUPPORTED_PROFILE.
                known_unsupported: name == KNOWN_UNSUPPORTED_PROFILE,
            })
            .collect())
    }

    fn active_profile_name(&self) -> Result<String, BackendError> {
        self.sysfs
            .read_to_string(std::path::Path::new(PLATFORM_PROFILE_PATH))
            .map(|raw| raw.trim().to_string())
            .map_err(map_sysfs_error)
    }

    fn set_active_profile(&self, name: &str) -> Result<(), BackendError> {
        self.sysfs
            .write_to_string(std::path::Path::new(PLATFORM_PROFILE_PATH), name)
            .map_err(map_sysfs_error)
    }
}

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::sync::Mutex;

    pub struct MockPowerProfilesBackend {
        state: Mutex<MockState>,
    }

    struct MockState {
        profiles: Result<Vec<ProfileInfo>, BackendError>,
        active: Result<String, BackendError>,
        set_result: Result<(), BackendError>,
        last_set_call: Option<String>,
    }

    impl MockPowerProfilesBackend {
        pub fn new(profiles: Vec<ProfileInfo>, active: &str) -> Self {
            Self {
                state: Mutex::new(MockState {
                    profiles: Ok(profiles),
                    active: Ok(active.to_string()),
                    set_result: Ok(()),
                    last_set_call: None,
                }),
            }
        }

        pub fn unavailable() -> Self {
            Self {
                state: Mutex::new(MockState {
                    profiles: Err(BackendError::Unavailable),
                    active: Err(BackendError::Unavailable),
                    set_result: Err(BackendError::Unavailable),
                    last_set_call: None,
                }),
            }
        }

        pub fn fail_set_with(&self, error: BackendError) {
            self.state.lock().unwrap().set_result = Err(error);
        }

        pub fn last_set_call(&self) -> Option<String> {
            self.state.lock().unwrap().last_set_call.clone()
        }
    }

    impl PowerProfilesBackend for MockPowerProfilesBackend {
        fn profiles(&self) -> Result<Vec<ProfileInfo>, BackendError> {
            self.state.lock().unwrap().profiles.clone()
        }

        fn active_profile_name(&self) -> Result<String, BackendError> {
            self.state.lock().unwrap().active.clone()
        }

        fn set_active_profile(&self, name: &str) -> Result<(), BackendError> {
            let mut state = self.state.lock().unwrap();
            state.last_set_call = Some(name.to_string());
            state.set_result.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mock::MockPowerProfilesBackend;

    mod zbus_profile_parsing {
        use super::*;
        use std::collections::HashMap;
        use zbus::zvariant::{OwnedValue, Value};

        fn dict(profile: &str, platform_driver: Option<&str>) -> HashMap<String, OwnedValue> {
            let mut d = HashMap::new();
            d.insert(
                "Profile".to_string(),
                OwnedValue::try_from(Value::new(profile)).unwrap(),
            );
            if let Some(driver) = platform_driver {
                d.insert(
                    "PlatformDriver".to_string(),
                    OwnedValue::try_from(Value::new(driver)).unwrap(),
                );
            }
            d
        }

        #[test]
        fn performance_backed_by_real_platform_profile_driver_is_known_unsupported() {
            // Once power-profiles-daemon adopts the real ACPI platform_profile
            // driver for `performance` (not the CpuDriver-only amd_pstate
            // path), it's the exact write M5 confirmed EIOs -- see
            // KNOWN_UNSUPPORTED_PROFILE's doc comment.
            let info = profile_info_from_dict(&dict("performance", Some("platform_profile")))
                .unwrap();

            assert!(info.known_unsupported);
        }

        #[test]
        fn performance_backed_by_cpu_driver_is_not_known_unsupported() {
            // docs/hardware.md line 62: the reference machine's default
            // state -- performance backed only by amd_pstate, genuinely
            // works, no real ACPI platform_profile involved.
            let info = profile_info_from_dict(&dict("performance", Some("amd_pstate"))).unwrap();

            assert!(!info.known_unsupported);
        }

        #[test]
        fn other_profile_names_are_never_known_unsupported() {
            let info = profile_info_from_dict(&dict("balanced", Some("platform_profile")))
                .unwrap();

            assert!(!info.known_unsupported);
        }
    }

    fn three_profiles() -> Vec<ProfileInfo> {
        vec![
            ProfileInfo {
                name: "power-saver".to_string(),
                is_placeholder: true,
                known_unsupported: false,
            },
            ProfileInfo {
                name: "balanced".to_string(),
                is_placeholder: true,
                known_unsupported: false,
            },
            ProfileInfo {
                name: "performance".to_string(),
                is_placeholder: false,
                known_unsupported: false,
            },
        ]
    }

    // ---- list_profiles ----

    #[test]
    fn list_profiles_returns_all_names_in_order() {
        let provider = PowerProfilesDaemon::new(MockPowerProfilesBackend::new(
            three_profiles(),
            "performance",
        ));

        assert_eq!(
            provider.list_profiles(),
            CapabilityState::Supported(vec![
                "power-saver".to_string(),
                "balanced".to_string(),
                "performance".to_string(),
            ])
        );
    }

    #[test]
    fn list_profile_details_returns_full_profile_info_in_order() {
        // The GUI needs known_unsupported per entry (issue #25), not just
        // names -- list_profiles() alone can't carry that.
        let provider = PowerProfilesDaemon::new(MockPowerProfilesBackend::new(
            three_profiles(),
            "performance",
        ));

        assert_eq!(
            provider.list_profile_details(),
            CapabilityState::Supported(three_profiles())
        );
    }

    #[test]
    fn list_profiles_unsupported_when_backend_unavailable() {
        let provider = PowerProfilesDaemon::new(MockPowerProfilesBackend::unavailable());

        assert_eq!(provider.list_profiles(), CapabilityState::Unsupported);
    }

    #[test]
    fn failed_backend_reports_unavailable_end_to_end() {
        let provider = PowerProfilesDaemon::new(FailedBackend(BackendError::Unavailable));

        assert_eq!(provider.list_profiles(), CapabilityState::Unsupported);
        assert_eq!(provider.current_profile(), CapabilityState::Unsupported);
        assert_eq!(
            provider.set_profile("balanced"),
            Err(ProfileError::BackendUnavailable)
        );
    }

    #[test]
    fn failed_backend_preserves_denied_rather_than_reporting_unavailable() {
        // Regression test for the Copilot-flagged bug: a real connect()
        // failure (e.g. AccessDenied) must not be misreported as "service
        // not installed".
        let provider = PowerProfilesDaemon::new(FailedBackend(BackendError::Denied));

        assert_eq!(provider.list_profiles(), CapabilityState::RequiresPrivilege);
        assert_eq!(
            provider.current_profile(),
            CapabilityState::RequiresPrivilege
        );
        assert_eq!(
            provider.set_profile("balanced"),
            Err(ProfileError::BackendDenied)
        );
    }

    #[test]
    fn failed_backend_preserves_other_rather_than_reporting_unavailable() {
        let provider = PowerProfilesDaemon::new(FailedBackend(BackendError::Other(
            "connection reset".to_string(),
        )));

        assert_eq!(provider.list_profiles(), CapabilityState::Unknown);
        assert_eq!(provider.current_profile(), CapabilityState::Unknown);
        assert_eq!(
            provider.set_profile("balanced"),
            Err(ProfileError::BackendFailed("connection reset".to_string()))
        );
    }

    // ---- current_profile ----

    #[test]
    fn current_profile_is_supported_and_hardware_backed_for_a_real_driver() {
        let provider = PowerProfilesDaemon::new(MockPowerProfilesBackend::new(
            three_profiles(),
            "performance",
        ));

        assert_eq!(
            provider.current_profile(),
            CapabilityState::Supported(ProfileStatus {
                name: "performance".to_string(),
                hardware_backed: true,
            })
        );
    }

    #[test]
    fn current_profile_is_hardware_dependent_for_a_placeholder_backed_profile() {
        // Per docs/hardware.md: balanced/power-saver run PPD's placeholder
        // driver on this machine — real profile name, but a no-op.
        let provider =
            PowerProfilesDaemon::new(MockPowerProfilesBackend::new(three_profiles(), "balanced"));

        assert_eq!(
            provider.current_profile(),
            CapabilityState::HardwareDependent(ProfileStatus {
                name: "balanced".to_string(),
                hardware_backed: false,
            })
        );
    }

    #[test]
    fn current_profile_unsupported_when_backend_unavailable() {
        let provider = PowerProfilesDaemon::new(MockPowerProfilesBackend::unavailable());

        assert_eq!(provider.current_profile(), CapabilityState::Unsupported);
    }

    // ---- set_profile ----

    #[test]
    fn set_profile_calls_backend_with_a_valid_name() {
        // Copilot review (PR #1): this test previously only checked the
        // return value, not that the backend was actually invoked with the
        // requested name — MockPowerProfilesBackend already records that,
        // so assert on it directly.
        let backend = std::sync::Arc::new(MockPowerProfilesBackend::new(
            three_profiles(),
            "performance",
        ));
        let provider = PowerProfilesDaemon::new(backend.clone());

        let result = provider.set_profile("balanced");

        assert_eq!(result, Ok(()));
        assert_eq!(backend.last_set_call(), Some("balanced".to_string()));
    }

    #[test]
    fn set_profile_rejects_a_name_not_in_list_profiles_per_cli_md() {
        let backend = MockPowerProfilesBackend::new(three_profiles(), "performance");
        let provider = PowerProfilesDaemon::new(backend);

        let result = provider.set_profile("turbo-nitro-mode");

        assert_eq!(
            result,
            Err(ProfileError::InvalidProfile {
                requested: "turbo-nitro-mode".to_string(),
                valid: vec![
                    "power-saver".to_string(),
                    "balanced".to_string(),
                    "performance".to_string(),
                ],
            })
        );
    }

    #[test]
    fn set_profile_does_not_call_the_backend_when_the_name_is_invalid() {
        // SAFE-002/SAFE-003: invalid input is rejected before any write is
        // attempted, not clamped or passed through.
        let backend = std::sync::Arc::new(MockPowerProfilesBackend::new(
            three_profiles(),
            "performance",
        ));
        let provider = PowerProfilesDaemon::new(backend.clone());

        let _ = provider.set_profile("turbo-nitro-mode");

        assert_eq!(backend.last_set_call(), None);
    }

    #[test]
    fn set_profile_rejects_a_known_unsupported_profile_per_issue_25() {
        let backend = MockPowerProfilesBackend::new(
            vec![
                ProfileInfo {
                    name: "balanced".to_string(),
                    is_placeholder: true,
                    known_unsupported: false,
                },
                ProfileInfo {
                    name: "performance".to_string(),
                    is_placeholder: false,
                    known_unsupported: true,
                },
            ],
            "balanced",
        );
        let provider = PowerProfilesDaemon::new(backend);

        let result = provider.set_profile("performance");

        assert_eq!(
            result,
            Err(ProfileError::KnownUnsupportedProfile {
                requested: "performance".to_string(),
            })
        );
    }

    #[test]
    fn set_profile_does_not_call_the_backend_for_a_known_unsupported_profile() {
        // SAFE-002/SAFE-003, same as an invalid name: a write already known
        // to fail is rejected before it's attempted, not attempted anyway.
        let backend = std::sync::Arc::new(MockPowerProfilesBackend::new(
            vec![
                ProfileInfo {
                    name: "balanced".to_string(),
                    is_placeholder: true,
                    known_unsupported: false,
                },
                ProfileInfo {
                    name: "performance".to_string(),
                    is_placeholder: false,
                    known_unsupported: true,
                },
            ],
            "balanced",
        ));
        let provider = PowerProfilesDaemon::new(backend.clone());

        let _ = provider.set_profile("performance");

        assert_eq!(backend.last_set_call(), None);
    }

    #[test]
    fn set_profile_reports_backend_failure_without_assuming_success() {
        let backend = MockPowerProfilesBackend::new(three_profiles(), "performance");
        backend.fail_set_with(BackendError::Other("dbus timeout".to_string()));
        let provider = PowerProfilesDaemon::new(backend);

        let result = provider.set_profile("balanced");

        assert_eq!(
            result,
            Err(ProfileError::BackendFailed("dbus timeout".to_string()))
        );
    }

    #[test]
    fn set_profile_denied_maps_to_backend_denied() {
        let backend = MockPowerProfilesBackend::new(three_profiles(), "performance");
        backend.fail_set_with(BackendError::Denied);
        let provider = PowerProfilesDaemon::new(backend);

        let result = provider.set_profile("balanced");

        assert_eq!(result, Err(ProfileError::BackendDenied));
    }

    // ---- AcerPlatformProfileBackend (M5, FR-007) ----
    // Per docs/architecture.md's M5 design section: a second
    // PowerProfilesBackend impl, backed by /sys/firmware/acpi/platform_profile
    // directly rather than D-Bus, so it plugs into PowerProfilesDaemon/
    // PowerProfileProvider unchanged.

    mod acer_platform_profile_backend {
        use super::*;
        use crate::sysfs::mock::MockSysfsReader;

        const PROFILE_PATH: &str = "/sys/firmware/acpi/platform_profile";
        const CHOICES_PATH: &str = "/sys/firmware/acpi/platform_profile_choices";

        #[test]
        fn profiles_reads_platform_profile_choices() {
            let sysfs = MockSysfsReader::new();
            sysfs.set_content(
                CHOICES_PATH,
                "low-power quiet balanced balanced-performance performance\n",
            );
            let backend = AcerPlatformProfileBackend::new(sysfs);

            let profiles = backend.profiles().unwrap();

            assert_eq!(
                profiles,
                vec![
                    ProfileInfo {
                        name: "low-power".to_string(),
                        is_placeholder: false,
                        known_unsupported: false,
                    },
                    ProfileInfo {
                        name: "quiet".to_string(),
                        is_placeholder: false,
                        known_unsupported: false,
                    },
                    ProfileInfo {
                        name: "balanced".to_string(),
                        is_placeholder: false,
                        known_unsupported: false,
                    },
                    ProfileInfo {
                        name: "balanced-performance".to_string(),
                        is_placeholder: false,
                        known_unsupported: false,
                    },
                    // The one confirmed-EIO value (M5, KNOWN_UNSUPPORTED_PROFILE) --
                    // this backend's mere existence implies real ACPI backing,
                    // so it's known_unsupported unconditionally.
                    ProfileInfo {
                        name: "performance".to_string(),
                        is_placeholder: false,
                        known_unsupported: true,
                    },
                ]
            );
        }

        #[test]
        fn profiles_unavailable_when_choices_file_absent_predator_v4_not_loaded() {
            let backend = AcerPlatformProfileBackend::new(MockSysfsReader::new());

            assert_eq!(backend.profiles(), Err(BackendError::Unavailable));
        }

        #[test]
        fn active_profile_name_reads_platform_profile() {
            let sysfs = MockSysfsReader::new();
            sysfs.set_content(PROFILE_PATH, "balanced\n");
            let backend = AcerPlatformProfileBackend::new(sysfs);

            assert_eq!(backend.active_profile_name(), Ok("balanced".to_string()));
        }

        #[test]
        fn active_profile_name_unavailable_when_file_absent() {
            let backend = AcerPlatformProfileBackend::new(MockSysfsReader::new());

            assert_eq!(
                backend.active_profile_name(),
                Err(BackendError::Unavailable)
            );
        }

        #[test]
        fn set_active_profile_writes_platform_profile() {
            let sysfs = MockSysfsReader::new();
            let backend = AcerPlatformProfileBackend::new(sysfs);

            let result = backend.set_active_profile("quiet");

            assert_eq!(result, Ok(()));
            assert_eq!(backend.active_profile_name(), Ok("quiet".to_string()));
        }

        #[test]
        fn set_active_profile_denied_when_write_permission_denied() {
            let sysfs = MockSysfsReader::new();
            sysfs.set_write_permission_denied(PROFILE_PATH);
            let backend = AcerPlatformProfileBackend::new(sysfs);

            assert_eq!(
                backend.set_active_profile("quiet"),
                Err(BackendError::Denied)
            );
        }

        #[test]
        fn set_active_profile_reports_the_real_eio_seen_writing_performance_in_m5() {
            let sysfs = MockSysfsReader::new();
            sysfs.set_write_failure(PROFILE_PATH, "Input/output error (os error 5)");
            let backend = AcerPlatformProfileBackend::new(sysfs);

            assert_eq!(
                backend.set_active_profile("performance"),
                Err(BackendError::Other(
                    "Input/output error (os error 5)".to_string()
                ))
            );
        }

        // Integration-style: confirms the new backend composes correctly
        // with PowerProfilesDaemon's existing validation/state-mapping
        // logic (already covered generically above), not just in isolation.

        #[test]
        fn through_power_profiles_daemon_unsupported_by_default_predator_v4_not_loaded() {
            let provider =
                PowerProfilesDaemon::new(AcerPlatformProfileBackend::new(MockSysfsReader::new()));

            assert_eq!(provider.list_profiles(), CapabilityState::Unsupported);
            assert_eq!(provider.current_profile(), CapabilityState::Unsupported);
        }

        #[test]
        fn through_power_profiles_daemon_rejects_invalid_profile_without_writing_safe_003() {
            let sysfs = MockSysfsReader::new();
            sysfs.set_content(
                CHOICES_PATH,
                "low-power quiet balanced balanced-performance performance\n",
            );
            let provider = PowerProfilesDaemon::new(AcerPlatformProfileBackend::new(sysfs));

            let result = provider.set_profile("turbo-nitro-mode");

            assert_eq!(
                result,
                Err(ProfileError::InvalidProfile {
                    requested: "turbo-nitro-mode".to_string(),
                    valid: vec![
                        "low-power".to_string(),
                        "quiet".to_string(),
                        "balanced".to_string(),
                        "balanced-performance".to_string(),
                        "performance".to_string(),
                    ],
                })
            );
        }

        #[test]
        fn through_power_profiles_daemon_set_valid_profile_writes_and_reports_success() {
            let sysfs = MockSysfsReader::new();
            sysfs.set_content(
                CHOICES_PATH,
                "low-power quiet balanced balanced-performance performance\n",
            );
            let provider = PowerProfilesDaemon::new(AcerPlatformProfileBackend::new(sysfs));

            assert_eq!(provider.set_profile("quiet"), Ok(()));
            assert_eq!(
                provider.current_profile(),
                CapabilityState::Supported(ProfileStatus {
                    name: "quiet".to_string(),
                    hardware_backed: true,
                })
            );
        }

        #[test]
        fn through_power_profiles_daemon_performance_rejected_known_unsupported_before_writing_issue_25(
        ) {
            // Supersedes the old "write is attempted, EIO reported" test:
            // per issue #25, this exact write is now known in advance to
            // fail (M5, KNOWN_UNSUPPORTED_PROFILE), so it's rejected
            // client-side and never reaches the sysfs write at all -- no
            // more raw `Input/output error (os error 5)` surfaced to the
            // user for this specific, well-understood case.
            let sysfs = MockSysfsReader::new();
            sysfs.set_content(
                CHOICES_PATH,
                "low-power quiet balanced balanced-performance performance\n",
            );
            sysfs.set_content(PROFILE_PATH, "balanced\n");
            // Still armed with a write failure to prove it's never reached.
            sysfs.set_write_failure(PROFILE_PATH, "Input/output error (os error 5)");
            let provider = PowerProfilesDaemon::new(AcerPlatformProfileBackend::new(sysfs));

            let result = provider.set_profile("performance");

            assert_eq!(
                result,
                Err(ProfileError::KnownUnsupportedProfile {
                    requested: "performance".to_string(),
                })
            );
            // State is unchanged -- confirms the write was never attempted,
            // not just that its failure was reported (SAFE-002/003).
            assert_eq!(
                provider.current_profile(),
                CapabilityState::Supported(ProfileStatus {
                    name: "balanced".to_string(),
                    hardware_backed: true,
                })
            );
        }
    }
}
