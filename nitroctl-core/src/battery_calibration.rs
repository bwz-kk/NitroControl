//! Battery calibration mode control (M7, FR-009) — reads/writes
//! `/sys/bus/wmi/drivers/acer-wmi-battery/calibration_mode` via `SysfsReader`.
//!
//! Same driver as `[battery_limit]` (M6) — `bwz-kk/acer-wmi-battery` — no new
//! out-of-tree dependency. Deliberately a **separate trait**, not a second
//! method on `BatteryLimitProvider`: per docs/architecture.md's M7 design
//! section, `set(true)` here is not a persistent setting but a maintenance
//! operation with a real side effect (per the driver's own documented
//! behavior: disables `health_mode`, charges to 100%, then does one full
//! discharge and one full recharge — reportedly 12+ hours, with no
//! completion signal from the driver). Conflating the two under one trait
//! would let a caller treat calibration as casually as a charge-cap toggle,
//! which it isn't.
//!
//! `AcerWmiBatteryBackend` (from `battery_limit`) implements this trait too
//! — same physical driver instance, same `SysfsReader`, just a second
//! attribute path — rather than a second backend struct duplicating
//! construction for no reason.
//!
//! NitroControl never builds, installs, or loads that module itself
//! (SAFE-001/SAFE-002) — absence of the sysfs node is the default state.

use std::path::Path;

use crate::battery_limit::AcerWmiBatteryBackend;
use crate::capability::CapabilityState;
use crate::sysfs::SysfsReader;

const CALIBRATION_MODE_PATH: &str = "/sys/bus/wmi/drivers/acer-wmi-battery/calibration_mode";

/// Mirrors `battery_limit::BatteryLimitError`'s shape, kept as its own type
/// rather than shared — see this module's doc comment for why calibration
/// isn't folded into that trait at all.
#[derive(Debug, Clone, PartialEq)]
pub enum CalibrationError {
    /// The driver isn't loaded — `calibration_mode` doesn't exist. The
    /// default state, since NitroControl never loads it itself.
    Unavailable,
    /// The write was rejected for lack of permission.
    Denied,
    /// Any other IO failure; the message is reported verbatim per SAFE-004.
    Failed(String),
}

/// `Send + Sync` for the same reason as `BatteryLimitProvider` — a
/// long-lived instance can be shared with a background polling thread.
pub trait BatteryCalibrationProvider: Send + Sync {
    /// `true` when a calibration cycle (full discharge + recharge) is
    /// currently enabled. See this module's doc comment: verifying that a
    /// full cycle actually completes correctly on this hardware is
    /// explicitly out of scope for M7 (docs/hardware.md records the
    /// narrower evidence standard) — this only reports the driver's own
    /// state.
    fn calibration_mode(&self) -> CapabilityState<bool>;
    fn set_calibration_mode(&self, enabled: bool) -> Result<(), CalibrationError>;
}

fn classify_io_error(err: std::io::Error) -> CalibrationError {
    match err.kind() {
        std::io::ErrorKind::NotFound => CalibrationError::Unavailable,
        std::io::ErrorKind::PermissionDenied => CalibrationError::Denied,
        _ => CalibrationError::Failed(err.to_string()),
    }
}

fn error_as_capability_state<T>(err: CalibrationError) -> CapabilityState<T> {
    match err {
        CalibrationError::Unavailable => CapabilityState::Unsupported,
        CalibrationError::Denied => CapabilityState::RequiresPrivilege,
        CalibrationError::Failed(_) => CapabilityState::Unknown,
    }
}

/// Parses the raw `calibration_mode` content. Same driver, same
/// `sprintf("%d\n", ...)` shape as `health_mode`
/// (`battery_limit::parse_health_mode`) — `0`/`1`/anything else (including
/// the driver's own "-1: not available on this battery" sentinel) maps the
/// same way.
fn parse_calibration_mode(raw: &str) -> CapabilityState<bool> {
    match raw.trim() {
        "0" => CapabilityState::Supported(false),
        "1" => CapabilityState::Supported(true),
        _ => CapabilityState::Unknown,
    }
}

impl<R: SysfsReader> BatteryCalibrationProvider for AcerWmiBatteryBackend<R> {
    fn calibration_mode(&self) -> CapabilityState<bool> {
        match self
            .sysfs()
            .read_to_string(Path::new(CALIBRATION_MODE_PATH))
        {
            Ok(raw) => parse_calibration_mode(&raw),
            Err(e) => error_as_capability_state(classify_io_error(e)),
        }
    }

    fn set_calibration_mode(&self, enabled: bool) -> Result<(), CalibrationError> {
        let content = if enabled { "1" } else { "0" };
        self.sysfs()
            .write_to_string(Path::new(CALIBRATION_MODE_PATH), content)
            .map_err(classify_io_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysfs::mock::MockSysfsReader;

    // ---- calibration_mode ----

    #[test]
    fn calibration_mode_supported_true_when_file_reads_1() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_content(CALIBRATION_MODE_PATH, "1\n");
        let backend = AcerWmiBatteryBackend::new(sysfs);

        assert_eq!(backend.calibration_mode(), CapabilityState::Supported(true));
    }

    #[test]
    fn calibration_mode_supported_false_when_file_reads_0() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_content(CALIBRATION_MODE_PATH, "0\n");
        let backend = AcerWmiBatteryBackend::new(sysfs);

        assert_eq!(
            backend.calibration_mode(),
            CapabilityState::Supported(false)
        );
    }

    #[test]
    fn calibration_mode_unsupported_when_driver_not_loaded() {
        let backend = AcerWmiBatteryBackend::new(MockSysfsReader::new());

        assert_eq!(backend.calibration_mode(), CapabilityState::Unsupported);
    }

    #[test]
    fn calibration_mode_unknown_for_the_driver_not_available_sentinel() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_content(CALIBRATION_MODE_PATH, "-1\n");
        let backend = AcerWmiBatteryBackend::new(sysfs);

        assert_eq!(backend.calibration_mode(), CapabilityState::Unknown);
    }

    // ---- set_calibration_mode ----

    #[test]
    fn set_calibration_mode_writes_1_for_enabled() {
        let sysfs = MockSysfsReader::new();
        let backend = AcerWmiBatteryBackend::new(sysfs);

        let result = backend.set_calibration_mode(true);

        assert_eq!(result, Ok(()));
        assert_eq!(backend.calibration_mode(), CapabilityState::Supported(true));
    }

    #[test]
    fn set_calibration_mode_writes_0_for_disabled() {
        let sysfs = MockSysfsReader::new();
        let backend = AcerWmiBatteryBackend::new(sysfs);

        let result = backend.set_calibration_mode(false);

        assert_eq!(result, Ok(()));
        assert_eq!(
            backend.calibration_mode(),
            CapabilityState::Supported(false)
        );
    }

    #[test]
    fn set_calibration_mode_denied_when_write_permission_denied() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_write_permission_denied(CALIBRATION_MODE_PATH);
        let backend = AcerWmiBatteryBackend::new(sysfs);

        assert_eq!(
            backend.set_calibration_mode(true),
            Err(CalibrationError::Denied)
        );
    }

    #[test]
    fn set_calibration_mode_failure_reported_verbatim_not_assumed_success() {
        // SAFE-004: a failed write is reported, and current state isn't
        // silently advanced as if the write had succeeded.
        let sysfs = MockSysfsReader::new();
        sysfs.set_content(CALIBRATION_MODE_PATH, "0\n");
        sysfs.set_write_failure(CALIBRATION_MODE_PATH, "Input/output error (os error 5)");
        let backend = AcerWmiBatteryBackend::new(sysfs);

        let result = backend.set_calibration_mode(true);

        assert_eq!(
            result,
            Err(CalibrationError::Failed(
                "Input/output error (os error 5)".to_string()
            ))
        );
        assert_eq!(
            backend.calibration_mode(),
            CapabilityState::Supported(false)
        );
    }

    #[test]
    fn health_mode_and_calibration_mode_are_independent_on_the_same_backend() {
        // One physical driver instance, two attributes -- writing one must
        // not disturb the other's state as seen through this struct.
        use crate::battery_limit::BatteryLimitProvider;

        let sysfs = MockSysfsReader::new();
        sysfs.set_content("/sys/bus/wmi/drivers/acer-wmi-battery/health_mode", "1\n");
        sysfs.set_content(CALIBRATION_MODE_PATH, "0\n");
        let backend = AcerWmiBatteryBackend::new(sysfs);

        backend.set_calibration_mode(true).unwrap();

        assert_eq!(backend.health_mode(), CapabilityState::Supported(true));
        assert_eq!(backend.calibration_mode(), CapabilityState::Supported(true));
    }
}
