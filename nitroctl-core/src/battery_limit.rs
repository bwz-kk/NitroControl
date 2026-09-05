//! Battery charge limit control (M6, FR-008) — reads/writes
//! `/sys/bus/wmi/drivers/acer-wmi-battery/health_mode` via `SysfsReader`.
//!
//! Exposed only by the out-of-tree
//! [`bwz-kk/acer-wmi-battery`](https://github.com/bwz-kk/acer-wmi-battery)
//! driver (a fork of `frederik-h/acer-wmi-battery` with an out-of-bounds
//! heap-read fix, docs/hardware.md) — the single, explicit exception to
//! this project's out-of-tree-independence stance, per
//! docs/architecture.md's "Battery charge limit (M6, FR-008)" section.
//! NitroControl never builds, installs, or loads that module itself
//! (SAFE-001/SAFE-002) — absence of the sysfs node is the default state.

use std::path::Path;

use crate::capability::CapabilityState;
use crate::sysfs::SysfsReader;

const HEALTH_MODE_PATH: &str = "/sys/bus/wmi/drivers/acer-wmi-battery/health_mode";

/// Mirrors `power_profile::BackendError`'s shape, kept separate rather than
/// shared: this feature has no D-Bus backend, and reusing that type would
/// pull in a name ("power profiles") this isn't about.
#[derive(Debug, Clone, PartialEq)]
pub enum BatteryLimitError {
    /// The driver isn't loaded — `health_mode` doesn't exist. The default
    /// state, since NitroControl never loads it itself.
    Unavailable,
    /// The write was rejected for lack of permission.
    Denied,
    /// Any other IO failure; the message is reported verbatim per SAFE-004.
    Failed(String),
}

/// `Send + Sync` for the same reason as `PowerProfileProvider` — a
/// long-lived instance can be shared with a background polling thread.
pub trait BatteryLimitProvider: Send + Sync {
    /// `true` when health mode (the ~80% charge cap) is enabled.
    fn health_mode(&self) -> CapabilityState<bool>;
    fn set_health_mode(&self, enabled: bool) -> Result<(), BatteryLimitError>;
}

fn classify_io_error(err: std::io::Error) -> BatteryLimitError {
    match err.kind() {
        std::io::ErrorKind::NotFound => BatteryLimitError::Unavailable,
        std::io::ErrorKind::PermissionDenied => BatteryLimitError::Denied,
        _ => BatteryLimitError::Failed(err.to_string()),
    }
}

fn error_as_capability_state<T>(err: BatteryLimitError) -> CapabilityState<T> {
    match err {
        BatteryLimitError::Unavailable => CapabilityState::Unsupported,
        BatteryLimitError::Denied => CapabilityState::RequiresPrivilege,
        BatteryLimitError::Failed(_) => CapabilityState::Unknown,
    }
}

/// Parses the raw `health_mode` content. The driver's `sprintf("%d\n", ...)`
/// (docs/hardware.md) only ever emits `0`/`1`/`-1` (the last meaning "not
/// available on this battery" per the driver's own semantics, which reads
/// the same as the sysfs node not existing at all from this trait's
/// perspective — surfaced as `Unknown` rather than guessed at, since it's a
/// real, distinct value this code hasn't independently confirmed on this
/// hardware).
fn parse_health_mode(raw: &str) -> CapabilityState<bool> {
    match raw.trim() {
        "0" => CapabilityState::Supported(false),
        "1" => CapabilityState::Supported(true),
        _ => CapabilityState::Unknown,
    }
}

/// Real backend: `bwz-kk/acer-wmi-battery`'s `health_mode` driver attribute.
pub struct AcerWmiBatteryBackend<R: SysfsReader> {
    sysfs: R,
}

impl<R: SysfsReader> AcerWmiBatteryBackend<R> {
    pub fn new(sysfs: R) -> Self {
        Self { sysfs }
    }

    /// Crate-internal accessor so `battery_calibration` (same physical
    /// driver instance, a different sysfs attribute) can implement
    /// `BatteryCalibrationProvider` for this struct without a second,
    /// duplicate backend type.
    pub(crate) fn sysfs(&self) -> &R {
        &self.sysfs
    }
}

impl<R: SysfsReader> BatteryLimitProvider for AcerWmiBatteryBackend<R> {
    fn health_mode(&self) -> CapabilityState<bool> {
        match self.sysfs.read_to_string(Path::new(HEALTH_MODE_PATH)) {
            Ok(raw) => parse_health_mode(&raw),
            Err(e) => error_as_capability_state(classify_io_error(e)),
        }
    }

    fn set_health_mode(&self, enabled: bool) -> Result<(), BatteryLimitError> {
        let content = if enabled { "1" } else { "0" };
        self.sysfs
            .write_to_string(Path::new(HEALTH_MODE_PATH), content)
            .map_err(classify_io_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysfs::mock::MockSysfsReader;

    // ---- health_mode ----

    #[test]
    fn health_mode_supported_true_when_file_reads_1() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_content(HEALTH_MODE_PATH, "1\n");
        let backend = AcerWmiBatteryBackend::new(sysfs);

        assert_eq!(backend.health_mode(), CapabilityState::Supported(true));
    }

    #[test]
    fn health_mode_supported_false_when_file_reads_0() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_content(HEALTH_MODE_PATH, "0\n");
        let backend = AcerWmiBatteryBackend::new(sysfs);

        assert_eq!(backend.health_mode(), CapabilityState::Supported(false));
    }

    #[test]
    fn health_mode_unsupported_when_driver_not_loaded() {
        let backend = AcerWmiBatteryBackend::new(MockSysfsReader::new());

        assert_eq!(backend.health_mode(), CapabilityState::Unsupported);
    }

    #[test]
    fn health_mode_unknown_for_the_driver_not_available_sentinel() {
        // The driver's own "-1: not available on this battery" case
        // (docs/hardware.md's read of acer-wmi-battery.c) -- distinct from
        // the node not existing at all, so it must not collapse to the same
        // Unsupported state.
        let sysfs = MockSysfsReader::new();
        sysfs.set_content(HEALTH_MODE_PATH, "-1\n");
        let backend = AcerWmiBatteryBackend::new(sysfs);

        assert_eq!(backend.health_mode(), CapabilityState::Unknown);
    }

    // ---- set_health_mode ----

    #[test]
    fn set_health_mode_writes_1_for_enabled() {
        let sysfs = MockSysfsReader::new();
        let backend = AcerWmiBatteryBackend::new(sysfs);

        let result = backend.set_health_mode(true);

        assert_eq!(result, Ok(()));
        assert_eq!(backend.health_mode(), CapabilityState::Supported(true));
    }

    #[test]
    fn set_health_mode_writes_0_for_disabled() {
        let sysfs = MockSysfsReader::new();
        let backend = AcerWmiBatteryBackend::new(sysfs);

        let result = backend.set_health_mode(false);

        assert_eq!(result, Ok(()));
        assert_eq!(backend.health_mode(), CapabilityState::Supported(false));
    }

    #[test]
    fn set_health_mode_denied_when_write_permission_denied() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_write_permission_denied(HEALTH_MODE_PATH);
        let backend = AcerWmiBatteryBackend::new(sysfs);

        assert_eq!(
            backend.set_health_mode(true),
            Err(BatteryLimitError::Denied)
        );
    }

    #[test]
    fn set_health_mode_failure_reported_verbatim_not_assumed_success() {
        // SAFE-004: a failed write is reported, and current state isn't
        // silently advanced as if the write had succeeded.
        let sysfs = MockSysfsReader::new();
        sysfs.set_content(HEALTH_MODE_PATH, "0\n");
        sysfs.set_write_failure(HEALTH_MODE_PATH, "Input/output error (os error 5)");
        let backend = AcerWmiBatteryBackend::new(sysfs);

        let result = backend.set_health_mode(true);

        assert_eq!(
            result,
            Err(BatteryLimitError::Failed(
                "Input/output error (os error 5)".to_string()
            ))
        );
        assert_eq!(backend.health_mode(), CapabilityState::Supported(false));
    }
}
