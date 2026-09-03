//! Power-profile control via `power-profiles-daemon`, per docs/architecture.md
//! and FR-005. `PowerProfilesBackend` is the testing seam (mirrors
//! `SysfsReader`/`CommandRunner`): all D-Bus IO lives behind it, so the
//! validation/state-mapping logic in `PowerProfilesDaemon` is unit-testable
//! without a live D-Bus connection.

use crate::capability::CapabilityState;

/// One entry of PPD's `Profiles` D-Bus property (verified shape via
/// `busctl introspect`, docs/hardware.md M3): each dict has a `Profile`
/// name and, only for a placeholder-backed profile, `PlatformDriver` ==
/// `"placeholder"`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileInfo {
    pub name: String,
    pub is_placeholder: bool,
}

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
    BackendUnavailable,
    BackendDenied,
    BackendFailed(String),
}

pub trait PowerProfileProvider {
    fn list_profiles(&self) -> CapabilityState<Vec<String>>;
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
        match self.backend.profiles() {
            Ok(profiles) => {
                CapabilityState::Supported(profiles.into_iter().map(|p| p.name).collect())
            }
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
        let valid: Vec<String> = profiles.into_iter().map(|p| p.name).collect();
        if !valid.iter().any(|name| name == profile) {
            return Err(ProfileError::InvalidProfile {
                requested: profile.to_string(),
                valid,
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

impl PowerProfilesBackend for ZbusPowerProfilesBackend {
    fn profiles(&self) -> Result<Vec<ProfileInfo>, BackendError> {
        let proxy = self.proxy()?;
        let raw: Vec<std::collections::HashMap<String, zbus::zvariant::OwnedValue>> =
            proxy.get_property("Profiles").map_err(map_zbus_error)?;

        raw.into_iter()
            .map(|dict| {
                let name = dict
                    .get("Profile")
                    .and_then(value_as_string)
                    .ok_or_else(|| {
                        BackendError::Other("Profiles entry missing 'Profile' name".to_string())
                    })?;
                let is_placeholder = dict
                    .get("PlatformDriver")
                    .and_then(value_as_string)
                    .map(|driver| driver == "placeholder")
                    .unwrap_or(false);
                Ok(ProfileInfo {
                    name,
                    is_placeholder,
                })
            })
            .collect()
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

    fn three_profiles() -> Vec<ProfileInfo> {
        vec![
            ProfileInfo {
                name: "power-saver".to_string(),
                is_placeholder: true,
            },
            ProfileInfo {
                name: "balanced".to_string(),
                is_placeholder: true,
            },
            ProfileInfo {
                name: "performance".to_string(),
                is_placeholder: false,
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
}
