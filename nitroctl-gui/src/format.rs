//! Pure formatting for dashboard rows, per docs/architecture.md: the GUI
//! never touches `/sys`/D-Bus/NVML directly, and never coerces an
//! unavailable `CapabilityState` to a fake value — it renders it as
//! explicit text and flags it so the window can style it distinctly
//! (dimmed subtitle) from a real reading.

use nitroctl_core::capability::CapabilityState;
use nitroctl_core::power_profile::ProfileStatus;
use nitroctl_core::sensor::{BatteryState, Celsius, Megahertz, MemoryUsage, Percent, Rpm};

/// What one dashboard row shows: the subtitle text, and whether it's a real
/// value (`true`) or an explicit "unavailable"/"unknown"/etc. state
/// (`false`) — never a fabricated number either way.
#[derive(Debug, Clone, PartialEq)]
pub struct RowContent {
    pub subtitle: String,
    pub available: bool,
}

fn describe<T>(state: &CapabilityState<T>, fmt: impl FnOnce(&T) -> String) -> RowContent {
    match state {
        CapabilityState::Supported(v) => RowContent {
            subtitle: fmt(v),
            available: true,
        },
        CapabilityState::Unsupported => RowContent {
            subtitle: "unavailable".to_string(),
            available: false,
        },
        CapabilityState::Unknown => RowContent {
            subtitle: "unknown".to_string(),
            available: false,
        },
        CapabilityState::RequiresPrivilege => RowContent {
            subtitle: "requires elevated privilege".to_string(),
            available: false,
        },
        CapabilityState::HardwareDependent(v) => RowContent {
            subtitle: format!("{} (hardware-dependent)", fmt(v)),
            available: false,
        },
    }
}

pub fn celsius(c: &Celsius) -> String {
    format!("{:.1}°C", c.0)
}

pub fn percent(p: &Percent) -> String {
    format!("{:.1}%", p.0)
}

pub fn megahertz(m: &Megahertz) -> String {
    format!("{:.0} MHz", m.0)
}

pub fn ram_usage(usage: &MemoryUsage) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    format!(
        "{:.1} GiB / {:.1} GiB",
        usage.used_bytes as f64 / GIB,
        usage.total_bytes as f64 / GIB
    )
}

pub fn battery(b: &BatteryState) -> String {
    let power = match b.power_watts {
        Some(w) => format!(", drawing {w:.1} W"),
        None => String::new(),
    };
    format!("{:.0}% ({:?}){power}", b.percent, b.status)
}

pub fn fan_rpms(rpms: &[Rpm]) -> String {
    rpms.iter()
        .map(|r| format!("{} RPM", r.0))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn profile_status(status: &ProfileStatus) -> String {
    status.name.clone()
}

pub fn cpu_temperature_row(state: &CapabilityState<Celsius>) -> RowContent {
    describe(state, celsius)
}

pub fn gpu_temperature_row(state: &CapabilityState<Celsius>) -> RowContent {
    describe(state, celsius)
}

pub fn cpu_utilization_row(state: &CapabilityState<Percent>) -> RowContent {
    describe(state, percent)
}

pub fn gpu_utilization_row(state: &CapabilityState<Percent>) -> RowContent {
    describe(state, percent)
}

pub fn cpu_frequency_row(state: &CapabilityState<Megahertz>) -> RowContent {
    describe(state, megahertz)
}

pub fn ram_usage_row(state: &CapabilityState<MemoryUsage>) -> RowContent {
    describe(state, ram_usage)
}

pub fn battery_row(state: &CapabilityState<BatteryState>) -> RowContent {
    describe(state, battery)
}

pub fn fan_rpm_row(state: &CapabilityState<Vec<Rpm>>) -> RowContent {
    describe(state, |rpms: &Vec<Rpm>| fan_rpms(rpms))
}

pub fn profile_status_row(state: &CapabilityState<ProfileStatus>) -> RowContent {
    describe(state, profile_status)
}

pub fn battery_limit(enabled: &bool) -> String {
    if *enabled { "on" } else { "off" }.to_string()
}

pub fn battery_limit_row(state: &CapabilityState<bool>) -> RowContent {
    describe(state, battery_limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nitroctl_core::sensor::BatteryStatus;

    // ---- describe() / RowContent, exercised via cpu_temperature_row ----

    #[test]
    fn supported_state_is_available_with_formatted_value() {
        let row = cpu_temperature_row(&CapabilityState::Supported(Celsius(55.8)));

        assert_eq!(
            row,
            RowContent {
                subtitle: "55.8°C".to_string(),
                available: true,
            }
        );
    }

    #[test]
    fn unsupported_state_is_unavailable() {
        let row = cpu_temperature_row(&CapabilityState::Unsupported);

        assert_eq!(
            row,
            RowContent {
                subtitle: "unavailable".to_string(),
                available: false,
            }
        );
    }

    #[test]
    fn unknown_state_is_unavailable() {
        let row = cpu_temperature_row(&CapabilityState::Unknown);

        assert_eq!(row.subtitle, "unknown");
        assert!(!row.available);
    }

    #[test]
    fn requires_privilege_state_is_unavailable() {
        let row = cpu_temperature_row(&CapabilityState::RequiresPrivilege);

        assert_eq!(row.subtitle, "requires elevated privilege");
        assert!(!row.available);
    }

    #[test]
    fn hardware_dependent_state_shows_the_value_but_flags_unavailable() {
        // Per capability.rs: HardwareDependent carries a real value, but it
        // doesn't necessarily reflect real hardware behavior — the GUI
        // shows the value (not just "unavailable") but still styles it as
        // not fully trustworthy, same convention as the CLI's exit code 1.
        let row = cpu_temperature_row(&CapabilityState::HardwareDependent(Celsius(42.0)));

        assert_eq!(row.subtitle, "42.0°C (hardware-dependent)");
        assert!(!row.available);
    }

    // ---- individual value formatters ----

    #[test]
    fn formats_percent_to_one_decimal() {
        assert_eq!(percent(&Percent(12.345)), "12.3%");
    }

    #[test]
    fn formats_megahertz_rounded() {
        assert_eq!(megahertz(&Megahertz(3563.9)), "3564 MHz");
    }

    #[test]
    fn formats_ram_usage_as_gib_used_of_total() {
        let usage = MemoryUsage {
            total_bytes: 16 * 1024 * 1024 * 1024,
            used_bytes: 8 * 1024 * 1024 * 1024,
        };
        assert_eq!(ram_usage(&usage), "8.0 GiB / 16.0 GiB");
    }

    #[test]
    fn formats_battery_with_power_draw() {
        let state = BatteryState {
            percent: 87.0,
            status: BatteryStatus::Discharging,
            power_watts: Some(34.5),
        };
        assert_eq!(battery(&state), "87% (Discharging), drawing 34.5 W");
    }

    #[test]
    fn formats_battery_without_power_draw() {
        let state = BatteryState {
            percent: 100.0,
            status: BatteryStatus::Full,
            power_watts: None,
        };
        assert_eq!(battery(&state), "100% (Full)");
    }

    #[test]
    fn formats_multiple_fan_rpms() {
        assert_eq!(fan_rpms(&[Rpm(2400), Rpm(2600)]), "2400 RPM, 2600 RPM");
    }

    #[test]
    fn formats_profile_status_name_only() {
        let status = ProfileStatus {
            name: "balanced".to_string(),
            hardware_backed: true,
        };
        assert_eq!(profile_status(&status), "balanced");
    }

    #[test]
    fn formats_battery_limit_on() {
        assert_eq!(battery_limit(&true), "on");
    }

    #[test]
    fn formats_battery_limit_off() {
        assert_eq!(battery_limit(&false), "off");
    }
}
