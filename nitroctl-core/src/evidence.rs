//! Raw evidence (paths/values) backing a `SensorProvider` reading, per
//! FR-006 (`docs/spec.md`) — `nitroctl diagnose` uses this to produce
//! GitHub-bug-report-ready output, not just a capability-state word.
//!
//! Scope (per FR-006's Acceptance Criteria, which only gates FR-001-006):
//! this trait covers `SensorProvider`'s metrics only, not FR-007/008/009's
//! Acer-specific providers (`acer-profile`/`battery-limit`/`battery-calibrate`)
//! — those have their own acceptance notes in spec.md with no evidence-path
//! requirement, so extending this to them is out of scope until a future
//! milestone decides otherwise.

use crate::sensor::{GpuKind, SensorProvider};

/// One metric's raw evidence: where the value came from, and what was
/// actually read there.
///
/// `raw_value` is populated whenever the underlying sysfs file or subprocess
/// was actually read, even if the value that came back was malformed or
/// implausible (i.e. even when the paired `CapabilityState` is `Unknown`) —
/// for a bug-report tool, the garbage reading itself is useful evidence, not
/// just a "malformed" state word. `None` only when there was no path/command
/// to point at in the first place (nothing found, i.e. `Unsupported`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    /// A sysfs path (e.g. `/sys/class/hwmon/hwmon5/temp1_input`) or a
    /// subprocess description (e.g. `nvidia-smi --query-gpu=...`).
    pub source: String,
    pub raw_value: Option<String>,
}

/// Redacts battery serial numbers and identifying DMI-style fields from raw
/// evidence text, per FR-006. Recognizes `KEY=value` lines (the format used
/// by `power_supply`/`dmi` sysfs `uevent` files) whose key ends in a known
/// identifying suffix, and replaces the value with `[REDACTED]`.
///
/// Deliberately suffix-matched (`_SERIAL`, `SERIAL_NUMBER`, `_UUID`,
/// `_ASSET_TAG`) rather than an exact list of today's known keys, since new
/// evidence sources (e.g. a future DMI board-serial read) must not silently
/// leak PII just because this function wasn't updated for them.
pub fn redact_evidence(raw: &str) -> String {
    const SENSITIVE_SUFFIXES: &[&str] = &["SERIAL_NUMBER", "_SERIAL", "_UUID", "_ASSET_TAG"];

    raw.lines()
        .map(|line| match line.split_once('=') {
            Some((key, _value))
                if SENSITIVE_SUFFIXES
                    .iter()
                    .any(|suffix| key.ends_with(suffix)) =>
            {
                format!("{key}=[REDACTED]")
            }
            _ => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parallel to `SensorProvider`: one method per metric, each returning the
/// raw evidence backing that metric's `SensorProvider` reading. Implemented
/// alongside `SensorProvider` by the same providers (`GenericLinux`,
/// `AcerNitroV15`) — kept as a separate trait rather than folded into
/// `SensorProvider` itself so `status`/`sensors`/`battery`/`fans` (which
/// only need the typed value) don't pay for evidence-string formatting on
/// every poll tick; only `diagnose` calls this.
pub trait EvidenceProvider: SensorProvider {
    fn cpu_temperature_evidence(&self) -> Option<Evidence>;
    fn gpu_temperature_evidence(&self, gpu: GpuKind) -> Option<Evidence>;
    fn cpu_utilization_evidence(&self) -> Option<Evidence>;
    fn gpu_utilization_evidence(&self, gpu: GpuKind) -> Option<Evidence>;
    fn cpu_frequency_evidence(&self) -> Option<Evidence>;
    fn ram_usage_evidence(&self) -> Option<Evidence>;
    fn battery_evidence(&self) -> Option<Evidence>;
    fn fan_rpm_evidence(&self) -> Option<Evidence>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_serial_number_key() {
        let raw = "POWER_SUPPLY_STATUS=Discharging\nPOWER_SUPPLY_SERIAL_NUMBER=ABC123XYZ\nPOWER_SUPPLY_CAPACITY=87";
        let redacted = redact_evidence(raw);

        assert!(redacted.contains("POWER_SUPPLY_SERIAL_NUMBER=[REDACTED]"));
        assert!(!redacted.contains("ABC123XYZ"));
        assert!(redacted.contains("POWER_SUPPLY_STATUS=Discharging"));
        assert!(redacted.contains("POWER_SUPPLY_CAPACITY=87"));
    }

    #[test]
    fn redacts_dmi_style_uuid_and_asset_tag_defensively() {
        // Not read by any provider today, but must not leak if one ever is.
        let raw =
            "PRODUCT_UUID=1234-5678\nBOARD_ASSET_TAG=Owner-Laptop-42\nPRODUCT_NAME=Nitro ANV15-41";
        let redacted = redact_evidence(raw);

        assert!(redacted.contains("PRODUCT_UUID=[REDACTED]"));
        assert!(redacted.contains("BOARD_ASSET_TAG=[REDACTED]"));
        assert!(redacted.contains("PRODUCT_NAME=Nitro ANV15-41")); // not identifying, kept
    }

    #[test]
    fn leaves_non_matching_lines_untouched() {
        let raw = "MemTotal:       16330000 kB\nMemAvailable:   10000000 kB";

        assert_eq!(redact_evidence(raw), raw);
    }

    #[test]
    fn handles_lines_with_no_equals_sign() {
        let raw = "cpu  100 0 100 800 0 0 0 0 0 0";

        assert_eq!(redact_evidence(raw), raw);
    }
}
