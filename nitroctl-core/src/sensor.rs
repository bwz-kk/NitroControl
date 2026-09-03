//! Sensor value types and the `SensorProvider` trait, per docs/architecture.md.

use crate::capability::CapabilityState;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Celsius(pub f64);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Percent(pub f64);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Megahertz(pub f64);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rpm(pub u32);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemoryUsage {
    pub total_bytes: u64,
    pub used_bytes: u64,
}

/// Which GPU a query targets. Modeled explicitly rather than assuming a
/// single GPU, since the ANV15-41 has both an AMD iGPU and an NVIDIA dGPU
/// (see docs/hardware.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuKind {
    Integrated,
    Discrete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryStatus {
    Charging,
    Discharging,
    Full,
    NotCharging,
    Unknown,
}

impl BatteryStatus {
    /// Parses the `POWER_SUPPLY_STATUS` value from a `power_supply/BAT*/uevent`
    /// file. An unrecognized value is `Unknown` rather than an error, since a
    /// battery reporting some status is still evidence the battery is present.
    pub fn parse(status: &str) -> BatteryStatus {
        match status.trim() {
            "Charging" => BatteryStatus::Charging,
            "Discharging" => BatteryStatus::Discharging,
            "Full" => BatteryStatus::Full,
            "Not charging" => BatteryStatus::NotCharging,
            _ => BatteryStatus::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BatteryState {
    pub percent: f64,
    pub status: BatteryStatus,
    /// Power draw in watts, derived as voltage * current when both are
    /// available. `None` when it can't be derived (per FR-003, never a fake 0).
    pub power_watts: Option<f64>,
}

/// Read-only hardware telemetry. Implemented once per supported machine
/// (`GenericLinux`, `AcerNitroV15`, ...) — see `crate::provider`.
/// `Send + Sync` so a long-lived provider can be shared (e.g. via `Arc`)
/// with a background polling thread — required for `nitroctl-gui`, whose
/// `cpu_utilization()` rate calculation only produces a real value across
/// two calls on the *same* provider instance (see `GenericLinux`'s internal
/// state), so the GUI must reuse one provider across polls rather than
/// building a fresh one each tick.
pub trait SensorProvider: Send + Sync {
    fn cpu_temperature(&self) -> CapabilityState<Celsius>;
    fn gpu_temperature(&self, gpu: GpuKind) -> CapabilityState<Celsius>;
    fn cpu_utilization(&self) -> CapabilityState<Percent>;
    fn gpu_utilization(&self, gpu: GpuKind) -> CapabilityState<Percent>;
    fn cpu_frequency(&self) -> CapabilityState<Megahertz>;
    fn ram_usage(&self) -> CapabilityState<MemoryUsage>;
    fn battery(&self) -> CapabilityState<BatteryState>;
    fn fan_rpm(&self) -> CapabilityState<Vec<Rpm>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_charging() {
        assert_eq!(BatteryStatus::parse("Charging"), BatteryStatus::Charging);
    }

    #[test]
    fn parses_discharging() {
        assert_eq!(
            BatteryStatus::parse("Discharging"),
            BatteryStatus::Discharging
        );
    }

    #[test]
    fn parses_full() {
        assert_eq!(BatteryStatus::parse("Full"), BatteryStatus::Full);
    }

    #[test]
    fn parses_not_charging() {
        assert_eq!(
            BatteryStatus::parse("Not charging"),
            BatteryStatus::NotCharging
        );
    }

    #[test]
    fn unrecognized_value_is_unknown_not_an_error() {
        assert_eq!(BatteryStatus::parse("Bogus"), BatteryStatus::Unknown);
    }

    #[test]
    fn trims_trailing_newline_from_sysfs_style_input() {
        assert_eq!(BatteryStatus::parse("Charging\n"), BatteryStatus::Charging);
    }
}
