//! CPU package power draw (M10, FR-010) — reads
//! `/sys/class/powercap/intel-rapl:0/energy_uj` via `SysfsReader`.
//!
//! Despite the "intel-rapl" name, this is the generic Linux RAPL-compatible
//! powercap interface (`CONFIG_POWERCAP`), not Intel-specific — confirmed
//! present and readable-by-root on this AMD machine (docs/hardware.md).
//! `energy_uj` is a monotonically increasing microjoule counter, not an
//! instantaneous power reading: average watts is derived from two samples
//! over a measured time delta, the same statefulness shape as
//! `SensorProvider::cpu_utilization`'s `/proc/stat` rate calculation.
//!
//! Two things set this apart from every other v1 sensor:
//! - **Root-only by default** on this machine (confirmed live: `energy_uj`
//!   is `-r-------- root:root`) — reports `RequiresPrivilege` until a user
//!   separately relaxes it (SAFE-001/002 stance, same as FR-007/FR-008;
//!   `docs/optional-setup.md` documents the copy-paste udev rule).
//! - **A very small wraparound range**: this zone's `max_energy_range_uj`
//!   is only ~65.5 J (`docs/hardware.md`), small enough to wrap within a
//!   couple of seconds under normal laptop package power — handled here by
//!   re-reading `max_energy_range_uj` and adding it back when the counter
//!   goes backwards, rather than assuming a fixed 32-bit range like some
//!   Intel RAPL zones.

use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use crate::capability::CapabilityState;
use crate::sensor::Watts;
use crate::sysfs::SysfsReader;

const CPU_PACKAGE_ENERGY_PATH: &str = "/sys/class/powercap/intel-rapl:0/energy_uj";
const CPU_PACKAGE_MAX_ENERGY_RANGE_PATH: &str =
    "/sys/class/powercap/intel-rapl:0/max_energy_range_uj";

#[derive(Debug, Clone, Copy)]
struct EnergySample {
    at: Instant,
    energy_uj: u64,
}

/// `Send + Sync` for the same reason as `SensorProvider`/`PowerProfileProvider`
/// — a long-lived instance can be shared with a background polling thread.
pub trait PowerDrawProvider: Send + Sync {
    /// Average CPU package power since the previous call, in watts. The
    /// first call on a fresh instance is always `Unknown` — there's no
    /// prior sample yet to derive a rate from (same shape as
    /// `SensorProvider::cpu_utilization`).
    fn cpu_package_power(&self) -> CapabilityState<Watts>;
}

/// Real backend: the generic RAPL-compatible `powercap` sysfs interface.
pub struct RaplPowerBackend<R: SysfsReader> {
    sysfs: R,
    history: Mutex<Option<EnergySample>>,
}

impl<R: SysfsReader> RaplPowerBackend<R> {
    pub fn new(sysfs: R) -> Self {
        Self {
            sysfs,
            history: Mutex::new(None),
        }
    }

    /// Same logic as the public `cpu_package_power`, but takes "now"
    /// explicitly so tests can drive the rate calculation with synthetic
    /// `Instant`s (`Instant + Duration` is real, deterministic arithmetic —
    /// no wall-clock sleep needed) instead of racing the real clock.
    fn cpu_package_power_at(&self, now: Instant) -> CapabilityState<Watts> {
        let raw = match self
            .sysfs
            .read_to_string(Path::new(CPU_PACKAGE_ENERGY_PATH))
        {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return CapabilityState::RequiresPrivilege;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return CapabilityState::Unsupported;
            }
            Err(_) => return CapabilityState::Unknown,
        };
        let Ok(energy_uj) = raw.trim().parse::<u64>() else {
            return CapabilityState::Unknown;
        };

        let mut history = self.history.lock().unwrap();
        let result = match *history {
            Some(prev) => self.watts_since(prev, energy_uj, now),
            None => CapabilityState::Unknown, // first sample: no baseline yet
        };
        *history = Some(EnergySample { at: now, energy_uj });
        result
    }

    fn watts_since(
        &self,
        prev: EnergySample,
        energy_uj: u64,
        now: Instant,
    ) -> CapabilityState<Watts> {
        let elapsed = now.saturating_duration_since(prev.at).as_secs_f64();
        if elapsed <= 0.0 {
            return CapabilityState::Unknown;
        }
        let delta_uj = if energy_uj >= prev.energy_uj {
            energy_uj - prev.energy_uj
        } else {
            // Counter wrapped since the last sample — read the range fresh
            // rather than caching it, in case it's ever wrong.
            let Some(range_uj) = self
                .sysfs
                .read_to_string(Path::new(CPU_PACKAGE_MAX_ENERGY_RANGE_PATH))
                .ok()
                .and_then(|r| r.trim().parse::<u64>().ok())
            else {
                return CapabilityState::Unknown;
            };
            range_uj.saturating_sub(prev.energy_uj) + energy_uj
        };
        let watts = (delta_uj as f64 / 1_000_000.0) / elapsed;
        CapabilityState::Supported(Watts(watts))
    }
}

impl<R: SysfsReader> PowerDrawProvider for RaplPowerBackend<R> {
    fn cpu_package_power(&self) -> CapabilityState<Watts> {
        self.cpu_package_power_at(Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysfs::mock::MockSysfsReader;
    use std::time::Duration;

    #[test]
    fn first_call_is_unknown_no_baseline_yet() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_content(CPU_PACKAGE_ENERGY_PATH, "1000000\n");
        let backend = RaplPowerBackend::new(sysfs);

        assert_eq!(backend.cpu_package_power(), CapabilityState::Unknown);
    }

    #[test]
    fn computes_watts_from_energy_delta_over_elapsed_time() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_sequence(
            CPU_PACKAGE_ENERGY_PATH,
            vec!["1000000\n".to_string(), "6000000\n".to_string()],
        );
        let backend = RaplPowerBackend::new(sysfs);
        let t0 = Instant::now();

        let _ = backend.cpu_package_power_at(t0); // establishes baseline
        let result = backend.cpu_package_power_at(t0 + Duration::from_millis(200));

        // delta = 5,000,000 uj = 5 J over 0.2s = 25 W
        match result {
            CapabilityState::Supported(Watts(w)) => assert!((w - 25.0).abs() < 0.001, "got {w}"),
            other => panic!("expected Supported, got {other:?}"),
        }
    }

    #[test]
    fn handles_counter_wraparound_using_max_energy_range() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_sequence(
            CPU_PACKAGE_ENERGY_PATH,
            vec!["60000000\n".to_string(), "5000000\n".to_string()],
        );
        sysfs.set_content(CPU_PACKAGE_MAX_ENERGY_RANGE_PATH, "65532610987\n");
        let backend = RaplPowerBackend::new(sysfs);
        let t0 = Instant::now();

        let _ = backend.cpu_package_power_at(t0);
        let result = backend.cpu_package_power_at(t0 + Duration::from_secs(1));

        // delta = (65532610987 - 60000000) + 5000000 = 65477610987 uj, over 1s
        match result {
            CapabilityState::Supported(Watts(w)) => {
                assert!((w - 65_477.610_987).abs() < 0.001, "got {w}");
            }
            other => panic!("expected Supported, got {other:?}"),
        }
    }

    #[test]
    fn wraparound_unknown_when_range_file_unreadable() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_sequence(
            CPU_PACKAGE_ENERGY_PATH,
            vec!["60000000\n".to_string(), "5000000\n".to_string()],
        );
        // max_energy_range_uj deliberately not set
        let backend = RaplPowerBackend::new(sysfs);
        let t0 = Instant::now();

        let _ = backend.cpu_package_power_at(t0);
        let result = backend.cpu_package_power_at(t0 + Duration::from_secs(1));

        assert_eq!(result, CapabilityState::Unknown);
    }

    #[test]
    fn requires_privilege_on_permission_denied() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_permission_denied(CPU_PACKAGE_ENERGY_PATH);
        let backend = RaplPowerBackend::new(sysfs);

        assert_eq!(
            backend.cpu_package_power(),
            CapabilityState::RequiresPrivilege
        );
    }

    #[test]
    fn unsupported_when_powercap_zone_missing() {
        let backend = RaplPowerBackend::new(MockSysfsReader::new());

        assert_eq!(backend.cpu_package_power(), CapabilityState::Unsupported);
    }

    #[test]
    fn unknown_on_malformed_value() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_content(CPU_PACKAGE_ENERGY_PATH, "not-a-number\n");
        let backend = RaplPowerBackend::new(sysfs);

        assert_eq!(backend.cpu_package_power(), CapabilityState::Unknown);
    }

    #[test]
    fn unknown_when_elapsed_time_is_zero() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_sequence(
            CPU_PACKAGE_ENERGY_PATH,
            vec!["1000000\n".to_string(), "2000000\n".to_string()],
        );
        let backend = RaplPowerBackend::new(sysfs);
        let t0 = Instant::now();

        let _ = backend.cpu_package_power_at(t0);
        let result = backend.cpu_package_power_at(t0); // same instant, zero elapsed

        assert_eq!(result, CapabilityState::Unknown);
    }
}
