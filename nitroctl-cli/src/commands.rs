//! Command implementations for `nitroctl`, per docs/cli.md.
//!
//! Every command is a pure function of a `&dyn SensorProvider`, returning
//! text to print and an exit code — no I/O, no process::exit here, so these
//! are testable with a fake provider (below) instead of real hardware.

use std::time::Duration;

use nitroctl_core::capability::CapabilityState;
use nitroctl_core::power_profile::{PowerProfileProvider, ProfileError, ProfileStatus};
use nitroctl_core::sensor::{GpuKind, MemoryUsage, Percent, Rpm, SensorProvider};

/// Per NFR-002, a bounded (not indefinite) pause: `cpu_utilization`'s rate
/// calculation needs two `/proc/stat` samples, but every CLI invocation is a
/// fresh process with no prior sample. One short, documented sleep — not a
/// blocking event loop — gets a real reading instead of always "unknown".
const CPU_UTILIZATION_SAMPLE_INTERVAL: Duration = Duration::from_millis(200);

fn sampled_cpu_utilization(provider: &dyn SensorProvider) -> CapabilityState<Percent> {
    let first = provider.cpu_utilization();
    if matches!(first, CapabilityState::Supported(_)) {
        return first;
    }
    std::thread::sleep(CPU_UTILIZATION_SAMPLE_INTERVAL);
    provider.cpu_utilization()
}

pub struct CommandOutput {
    pub text: String,
    pub exit_code: i32,
}

/// Renders one `CapabilityState<T>` as CLI text, per cli.md's output
/// conventions: `Supported` renders `fmt(value)`; every other state renders
/// its own explicit word, never a fabricated value. Returns the exit code
/// cli.md defines for a single-capability command (0 for Supported, 1
/// otherwise).
fn describe<T>(state: &CapabilityState<T>, fmt: impl FnOnce(&T) -> String) -> (String, i32) {
    match state {
        CapabilityState::Supported(v) => (fmt(v), 0),
        CapabilityState::Unsupported => ("unavailable".to_string(), 1),
        CapabilityState::Unknown => ("unknown".to_string(), 1),
        CapabilityState::RequiresPrivilege => ("requires elevated privilege".to_string(), 1),
        CapabilityState::HardwareDependent(v) => (format!("{} (hardware-dependent)", fmt(v)), 1),
    }
}

/// The state-name word `diagnose` prints for a capability, independent of
/// whether it's currently `Supported`.
fn state_label<T>(state: &CapabilityState<T>) -> &'static str {
    match state {
        CapabilityState::Supported(_) => "SUPPORTED",
        CapabilityState::Unsupported => "UNSUPPORTED",
        CapabilityState::Unknown => "UNKNOWN",
        CapabilityState::RequiresPrivilege => "REQUIRES_PRIVILEGE",
        CapabilityState::HardwareDependent(_) => "HARDWARE_DEPENDENT",
    }
}

fn format_celsius(c: &nitroctl_core::sensor::Celsius) -> String {
    format!("{:.1}°C", c.0)
}

fn format_percent(p: &nitroctl_core::sensor::Percent) -> String {
    format!("{:.1}%", p.0)
}

fn format_megahertz(m: &nitroctl_core::sensor::Megahertz) -> String {
    format!("{:.0} MHz", m.0)
}

fn format_ram(usage: &MemoryUsage) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    format!(
        "{:.1} GiB / {:.1} GiB",
        usage.used_bytes as f64 / GIB,
        usage.total_bytes as f64 / GIB
    )
}

fn format_battery(b: &nitroctl_core::sensor::BatteryState) -> String {
    let power = match b.power_watts {
        Some(w) => format!(", drawing {w:.1} W"),
        None => String::new(),
    };
    format!("{:.0}% ({:?}){power}", b.percent, b.status)
}

fn format_fan_rpms(rpms: &[Rpm]) -> String {
    rpms.iter()
        .map(|r| format!("{} RPM", r.0))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Appends one `label: value` line for `sensors`/`status` — always prints,
/// regardless of capability state, per cli.md's "unavailable, not omitted"
/// convention. Multi-metric commands don't fail on an individual
/// Unsupported/Unknown metric; that's documented normal output.
fn push_metric_line<T>(
    lines: &mut Vec<String>,
    label: &str,
    state: &CapabilityState<T>,
    fmt: impl FnOnce(&T) -> String,
) {
    let (value, _exit_code) = describe(state, fmt);
    lines.push(format!("{label}: {value}"));
}

fn sensor_lines(provider: &dyn SensorProvider) -> Vec<String> {
    let mut lines = Vec::new();
    push_metric_line(
        &mut lines,
        "CPU temperature",
        &provider.cpu_temperature(),
        format_celsius,
    );
    push_metric_line(
        &mut lines,
        "iGPU temperature",
        &provider.gpu_temperature(GpuKind::Integrated),
        format_celsius,
    );
    push_metric_line(
        &mut lines,
        "dGPU temperature",
        &provider.gpu_temperature(GpuKind::Discrete),
        format_celsius,
    );
    push_metric_line(
        &mut lines,
        "CPU frequency",
        &provider.cpu_frequency(),
        format_megahertz,
    );
    push_metric_line(
        &mut lines,
        "CPU utilization",
        &sampled_cpu_utilization(provider),
        format_percent,
    );
    push_metric_line(&mut lines, "RAM usage", &provider.ram_usage(), format_ram);
    lines
}

fn battery_line(provider: &dyn SensorProvider) -> (String, i32) {
    let state = provider.battery();
    let (value, exit_code) = describe(&state, format_battery);
    (format!("Battery: {value}"), exit_code)
}

pub fn run_sensors(provider: &dyn SensorProvider) -> CommandOutput {
    CommandOutput {
        text: sensor_lines(provider).join("\n"),
        exit_code: 0,
    }
}

pub fn run_status(provider: &dyn SensorProvider) -> CommandOutput {
    let mut lines = sensor_lines(provider);
    let (battery_text, _exit_code) = battery_line(provider);
    lines.push(battery_text);
    CommandOutput {
        text: lines.join("\n"),
        exit_code: 0,
    }
}

pub fn run_battery(provider: &dyn SensorProvider) -> CommandOutput {
    let (text, exit_code) = battery_line(provider);
    CommandOutput { text, exit_code }
}

pub fn run_fans(provider: &dyn SensorProvider) -> CommandOutput {
    let state = provider.fan_rpm();
    let (value, exit_code) = describe(&state, |rpms: &Vec<Rpm>| format_fan_rpms(rpms));
    CommandOutput {
        text: format!("Fan RPM: {value}"),
        exit_code,
    }
}

pub fn run_diagnose(provider: &dyn SensorProvider) -> CommandOutput {
    let mut lines = vec!["NitroControl diagnostic report".to_string()];

    let mut labeled = |label: &str, value: String, state_word: &str| {
        // SUPPORTED and HARDWARE_DEPENDENT both carry a real value; the
        // other three states don't, so there's nothing to show alongside
        // the state word for them.
        if state_word == "SUPPORTED" || state_word == "HARDWARE_DEPENDENT" {
            lines.push(format!("{label}: {state_word} ({value})"));
        } else {
            lines.push(format!("{label}: {state_word}"));
        }
    };

    let cpu_temp = provider.cpu_temperature();
    labeled(
        "CPU temperature",
        describe(&cpu_temp, format_celsius).0,
        state_label(&cpu_temp),
    );
    let igpu_temp = provider.gpu_temperature(GpuKind::Integrated);
    labeled(
        "iGPU temperature",
        describe(&igpu_temp, format_celsius).0,
        state_label(&igpu_temp),
    );
    let dgpu_temp = provider.gpu_temperature(GpuKind::Discrete);
    labeled(
        "dGPU temperature",
        describe(&dgpu_temp, format_celsius).0,
        state_label(&dgpu_temp),
    );
    let cpu_freq = provider.cpu_frequency();
    labeled(
        "CPU frequency",
        describe(&cpu_freq, format_megahertz).0,
        state_label(&cpu_freq),
    );
    let cpu_util = sampled_cpu_utilization(provider);
    labeled(
        "CPU utilization",
        describe(&cpu_util, format_percent).0,
        state_label(&cpu_util),
    );
    let igpu_util = provider.gpu_utilization(GpuKind::Integrated);
    labeled(
        "iGPU utilization",
        describe(&igpu_util, format_percent).0,
        state_label(&igpu_util),
    );
    let dgpu_util = provider.gpu_utilization(GpuKind::Discrete);
    labeled(
        "dGPU utilization",
        describe(&dgpu_util, format_percent).0,
        state_label(&dgpu_util),
    );
    let ram = provider.ram_usage();
    labeled("RAM usage", describe(&ram, format_ram).0, state_label(&ram));
    let battery = provider.battery();
    labeled(
        "Battery",
        describe(&battery, format_battery).0,
        state_label(&battery),
    );
    let fans = provider.fan_rpm();
    labeled(
        "Fan RPM",
        describe(&fans, |rpms: &Vec<Rpm>| format_fan_rpms(rpms)).0,
        state_label(&fans),
    );

    CommandOutput {
        text: lines.join("\n"),
        exit_code: 0,
    }
}

fn format_profile_status(status: &ProfileStatus) -> String {
    status.name.clone()
}

pub fn run_profile_list(provider: &dyn PowerProfileProvider) -> CommandOutput {
    let state = provider.list_profiles();
    let (value, exit_code) = describe(&state, |names: &Vec<String>| names.join(", "));
    CommandOutput {
        text: format!("Profiles: {value}"),
        exit_code,
    }
}

pub fn run_profile_get(provider: &dyn PowerProfileProvider) -> CommandOutput {
    let state = provider.current_profile();
    let (value, exit_code) = describe(&state, format_profile_status);
    CommandOutput {
        text: format!("Power profile: {value}"),
        exit_code,
    }
}

pub fn run_profile_set(provider: &dyn PowerProfileProvider, name: &str) -> CommandOutput {
    match provider.set_profile(name) {
        Ok(()) => CommandOutput {
            text: format!("Power profile set to {name}"),
            exit_code: 0,
        },
        Err(ProfileError::InvalidProfile { requested, valid }) => CommandOutput {
            text: format!(
                "Invalid profile {requested:?}; valid choices: {}",
                valid.join(", ")
            ),
            exit_code: 2,
        },
        Err(ProfileError::BackendUnavailable) => CommandOutput {
            text: "power-profiles-daemon is not available".to_string(),
            exit_code: 3,
        },
        Err(ProfileError::BackendDenied) => CommandOutput {
            text: "power-profiles-daemon denied the request".to_string(),
            exit_code: 3,
        },
        Err(ProfileError::BackendFailed(message)) => CommandOutput {
            text: format!("power-profiles-daemon call failed: {message}"),
            exit_code: 3,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nitroctl_core::capability::CapabilityState;
    use nitroctl_core::sensor::{BatteryState, BatteryStatus, Celsius, Megahertz, Percent};

    /// A hand-built `SensorProvider` for CLI tests — no filesystem or
    /// subprocess mocking needed, since the CLI only depends on the trait.
    struct FakeProvider {
        cpu_temperature: CapabilityState<Celsius>,
        gpu_temperature_integrated: CapabilityState<Celsius>,
        gpu_temperature_discrete: CapabilityState<Celsius>,
        /// A queue so tests can simulate the stateful two-call rate
        /// calculation `GenericLinux::cpu_utilization` actually has: pops
        /// one state per call, repeating the last once exhausted.
        cpu_utilization: std::cell::RefCell<std::collections::VecDeque<CapabilityState<Percent>>>,
        gpu_utilization_integrated: CapabilityState<Percent>,
        gpu_utilization_discrete: CapabilityState<Percent>,
        cpu_frequency: CapabilityState<Megahertz>,
        ram_usage: CapabilityState<MemoryUsage>,
        battery: CapabilityState<BatteryState>,
        fan_rpm: CapabilityState<Vec<Rpm>>,
    }

    impl FakeProvider {
        fn with_cpu_utilization_sequence(states: Vec<CapabilityState<Percent>>) -> Self {
            Self {
                cpu_utilization: std::cell::RefCell::new(states.into()),
                ..Default::default()
            }
        }
    }

    impl Default for FakeProvider {
        fn default() -> Self {
            Self {
                cpu_temperature: CapabilityState::Unsupported,
                gpu_temperature_integrated: CapabilityState::Unsupported,
                gpu_temperature_discrete: CapabilityState::Unsupported,
                cpu_utilization: std::cell::RefCell::new(vec![CapabilityState::Unsupported].into()),
                gpu_utilization_integrated: CapabilityState::Unsupported,
                gpu_utilization_discrete: CapabilityState::Unsupported,
                cpu_frequency: CapabilityState::Unsupported,
                ram_usage: CapabilityState::Unsupported,
                battery: CapabilityState::Unsupported,
                fan_rpm: CapabilityState::Unsupported,
            }
        }
    }

    impl SensorProvider for FakeProvider {
        fn cpu_temperature(&self) -> CapabilityState<Celsius> {
            self.cpu_temperature.clone()
        }
        fn gpu_temperature(&self, gpu: GpuKind) -> CapabilityState<Celsius> {
            match gpu {
                GpuKind::Integrated => self.gpu_temperature_integrated.clone(),
                GpuKind::Discrete => self.gpu_temperature_discrete.clone(),
            }
        }
        fn cpu_utilization(&self) -> CapabilityState<Percent> {
            let mut queue = self.cpu_utilization.borrow_mut();
            if queue.len() > 1 {
                queue.pop_front().unwrap()
            } else {
                queue.front().cloned().unwrap()
            }
        }
        fn gpu_utilization(&self, gpu: GpuKind) -> CapabilityState<Percent> {
            match gpu {
                GpuKind::Integrated => self.gpu_utilization_integrated.clone(),
                GpuKind::Discrete => self.gpu_utilization_discrete.clone(),
            }
        }
        fn cpu_frequency(&self) -> CapabilityState<Megahertz> {
            self.cpu_frequency.clone()
        }
        fn ram_usage(&self) -> CapabilityState<MemoryUsage> {
            self.ram_usage.clone()
        }
        fn battery(&self) -> CapabilityState<BatteryState> {
            self.battery.clone()
        }
        fn fan_rpm(&self) -> CapabilityState<Vec<Rpm>> {
            self.fan_rpm.clone()
        }
    }

    struct FakePowerProfileProvider {
        list_profiles: CapabilityState<Vec<String>>,
        current_profile: CapabilityState<ProfileStatus>,
        set_result: Result<(), ProfileError>,
    }

    impl Default for FakePowerProfileProvider {
        fn default() -> Self {
            Self {
                list_profiles: CapabilityState::Unsupported,
                current_profile: CapabilityState::Unsupported,
                set_result: Ok(()),
            }
        }
    }

    impl PowerProfileProvider for FakePowerProfileProvider {
        fn list_profiles(&self) -> CapabilityState<Vec<String>> {
            self.list_profiles.clone()
        }
        fn current_profile(&self) -> CapabilityState<ProfileStatus> {
            self.current_profile.clone()
        }
        fn set_profile(&self, _profile: &str) -> Result<(), ProfileError> {
            self.set_result.clone()
        }
    }

    // ---- run_battery ----

    #[test]
    fn battery_supported_prints_percent_status_and_power() {
        let provider = FakeProvider {
            battery: CapabilityState::Supported(BatteryState {
                percent: 87.0,
                status: BatteryStatus::Discharging,
                power_watts: Some(34.5),
            }),
            ..Default::default()
        };

        let out = run_battery(&provider);

        assert_eq!(out.exit_code, 0);
        assert!(out.text.contains("87%"), "{}", out.text);
        assert!(out.text.contains("Discharging"), "{}", out.text);
        assert!(out.text.contains("34.5 W"), "{}", out.text);
    }

    #[test]
    fn battery_unsupported_prints_unavailable_and_exits_1() {
        let provider = FakeProvider::default();

        let out = run_battery(&provider);

        assert_eq!(out.exit_code, 1);
        assert!(out.text.contains("unavailable"), "{}", out.text);
    }

    #[test]
    fn battery_requires_privilege_prints_that_word_and_exits_1() {
        let provider = FakeProvider {
            battery: CapabilityState::RequiresPrivilege,
            ..Default::default()
        };

        let out = run_battery(&provider);

        assert_eq!(out.text, "Battery: requires elevated privilege");
        assert_eq!(out.exit_code, 1);
    }

    #[test]
    fn battery_hardware_dependent_prints_the_value_with_a_caveat_and_exits_1() {
        // HardwareDependent still carries a real value (e.g. a placeholder
        // power-profile backend still names a real active profile) — losing
        // it would be a regression, so the CLI must show both the value and
        // the caveat, not just the bare word.
        let provider = FakeProvider {
            battery: CapabilityState::HardwareDependent(BatteryState {
                percent: 55.0,
                status: BatteryStatus::Discharging,
                power_watts: None,
            }),
            ..Default::default()
        };

        let out = run_battery(&provider);

        assert_eq!(out.text, "Battery: 55% (Discharging) (hardware-dependent)");
        assert_eq!(out.exit_code, 1);
    }

    #[test]
    fn battery_with_no_power_draw_omits_the_watts_clause() {
        let provider = FakeProvider {
            battery: CapabilityState::Supported(BatteryState {
                percent: 100.0,
                status: BatteryStatus::Full,
                power_watts: None,
            }),
            ..Default::default()
        };

        let out = run_battery(&provider);

        assert_eq!(out.exit_code, 0);
        assert!(!out.text.contains(" W"), "{}", out.text);
    }

    // ---- run_fans ----

    #[test]
    fn fans_unsupported_prints_unavailable_per_fr_004() {
        let provider = FakeProvider::default();

        let out = run_fans(&provider);

        assert_eq!(out.text, "Fan RPM: unavailable");
        assert_eq!(out.exit_code, 1);
    }

    #[test]
    fn fans_supported_lists_each_fan_rpm() {
        let provider = FakeProvider {
            fan_rpm: CapabilityState::Supported(vec![Rpm(2400), Rpm(2600)]),
            ..Default::default()
        };

        let out = run_fans(&provider);

        assert_eq!(out.text, "Fan RPM: 2400 RPM, 2600 RPM");
        assert_eq!(out.exit_code, 0);
    }

    // ---- run_sensors ----

    #[test]
    fn sensors_prints_every_metric_with_unit_and_exits_0_even_when_mixed() {
        let provider = FakeProvider {
            cpu_temperature: CapabilityState::Supported(Celsius(55.8)),
            fan_rpm: CapabilityState::Unsupported, // not part of `sensors`, but mixed state elsewhere
            gpu_temperature_discrete: CapabilityState::Unknown,
            ..Default::default()
        };

        let out = run_sensors(&provider);

        assert!(out.text.contains("CPU temperature: 55.8°C"), "{}", out.text);
        assert!(
            out.text.contains("dGPU temperature: unknown"),
            "{}",
            out.text
        );
        // A mix of available/unavailable metrics is documented normal
        // behavior for a multi-metric command, not a command failure.
        assert_eq!(out.exit_code, 0);
    }

    #[test]
    fn sensors_takes_a_second_cpu_utilization_sample_when_first_call_has_no_baseline() {
        // A CLI invocation is a fresh process each time, so the provider's
        // first cpu_utilization() call never has a prior sample to diff
        // against (see GenericLinux::cpu_utilization) — the command must
        // sample twice to report a real percentage instead of "unknown".
        let provider = FakeProvider::with_cpu_utilization_sequence(vec![
            CapabilityState::Unknown,
            CapabilityState::Supported(Percent(42.0)),
        ]);

        let out = run_sensors(&provider);

        assert!(out.text.contains("CPU utilization: 42.0%"), "{}", out.text);
    }

    #[test]
    fn sensors_skips_the_sleep_when_first_cpu_utilization_call_is_already_supported() {
        let provider =
            FakeProvider::with_cpu_utilization_sequence(vec![CapabilityState::Supported(Percent(
                10.0,
            ))]);

        let start = std::time::Instant::now();
        let out = run_sensors(&provider);

        assert!(
            start.elapsed() < CPU_UTILIZATION_SAMPLE_INTERVAL,
            "should not sleep when the first sample is already Supported"
        );
        assert!(out.text.contains("CPU utilization: 10.0%"), "{}", out.text);
    }

    // ---- run_status ----

    #[test]
    fn status_includes_sensor_and_battery_lines() {
        let provider = FakeProvider {
            cpu_temperature: CapabilityState::Supported(Celsius(50.0)),
            battery: CapabilityState::Supported(BatteryState {
                percent: 90.0,
                status: BatteryStatus::Charging,
                power_watts: None,
            }),
            ..Default::default()
        };

        let out = run_status(&provider);

        assert!(out.text.contains("CPU temperature: 50.0°C"), "{}", out.text);
        assert!(out.text.contains("Battery: 90%"), "{}", out.text);
        assert_eq!(out.exit_code, 0);
    }

    // ---- run_diagnose ----

    #[test]
    fn diagnose_labels_every_metric_with_its_capability_state() {
        let provider = FakeProvider {
            cpu_temperature: CapabilityState::Supported(Celsius(55.8)),
            fan_rpm: CapabilityState::Unsupported,
            ..Default::default()
        };

        let out = run_diagnose(&provider);

        assert!(
            out.text.contains("CPU temperature: SUPPORTED (55.8°C)"),
            "{}",
            out.text
        );
        assert!(out.text.contains("Fan RPM: UNSUPPORTED"), "{}", out.text);
        assert_eq!(out.exit_code, 0);
    }

    #[test]
    fn diagnose_shows_the_value_for_hardware_dependent_metrics_too() {
        // Not just the state word — HardwareDependent carries a real value
        // (e.g. a placeholder power-profile backend's active profile name),
        // so diagnose must not discard it the way it discards Unsupported's
        // (nonexistent) value.
        let provider = FakeProvider {
            battery: CapabilityState::HardwareDependent(BatteryState {
                percent: 55.0,
                status: BatteryStatus::Discharging,
                power_watts: None,
            }),
            ..Default::default()
        };

        let out = run_diagnose(&provider);

        assert!(
            out.text
                .contains("Battery: HARDWARE_DEPENDENT (55% (Discharging) (hardware-dependent))"),
            "{}",
            out.text
        );
    }

    // ---- run_profile_list ----

    #[test]
    fn profile_list_prints_comma_separated_names() {
        let provider = FakePowerProfileProvider {
            list_profiles: CapabilityState::Supported(vec![
                "power-saver".to_string(),
                "balanced".to_string(),
                "performance".to_string(),
            ]),
            ..Default::default()
        };

        let out = run_profile_list(&provider);

        assert_eq!(out.text, "Profiles: power-saver, balanced, performance");
        assert_eq!(out.exit_code, 0);
    }

    #[test]
    fn profile_list_unsupported_when_ppd_unavailable() {
        let provider = FakePowerProfileProvider::default();

        let out = run_profile_list(&provider);

        assert_eq!(out.text, "Profiles: unavailable");
        assert_eq!(out.exit_code, 1);
    }

    // ---- run_profile_get ----

    #[test]
    fn profile_get_prints_the_active_profile_name() {
        let provider = FakePowerProfileProvider {
            current_profile: CapabilityState::Supported(ProfileStatus {
                name: "performance".to_string(),
                hardware_backed: true,
            }),
            ..Default::default()
        };

        let out = run_profile_get(&provider);

        assert_eq!(out.text, "Power profile: performance");
        assert_eq!(out.exit_code, 0);
    }

    #[test]
    fn profile_get_flags_a_placeholder_backed_profile() {
        let provider = FakePowerProfileProvider {
            current_profile: CapabilityState::HardwareDependent(ProfileStatus {
                name: "balanced".to_string(),
                hardware_backed: false,
            }),
            ..Default::default()
        };

        let out = run_profile_get(&provider);

        assert_eq!(out.text, "Power profile: balanced (hardware-dependent)");
        assert_eq!(out.exit_code, 1);
    }

    // ---- run_profile_set ----

    #[test]
    fn profile_set_success_prints_confirmation_and_exits_0() {
        let provider = FakePowerProfileProvider {
            set_result: Ok(()),
            ..Default::default()
        };

        let out = run_profile_set(&provider, "balanced");

        assert_eq!(out.text, "Power profile set to balanced");
        assert_eq!(out.exit_code, 0);
    }

    #[test]
    fn profile_set_invalid_name_names_the_valid_choices_and_exits_2() {
        // Per cli.md: "no silent clamping to a nearby valid value" and
        // exit code 2 for an invalid argument.
        let provider = FakePowerProfileProvider {
            set_result: Err(ProfileError::InvalidProfile {
                requested: "turbo-nitro-mode".to_string(),
                valid: vec!["power-saver".to_string(), "balanced".to_string()],
            }),
            ..Default::default()
        };

        let out = run_profile_set(&provider, "turbo-nitro-mode");

        assert!(out.text.contains("turbo-nitro-mode"), "{}", out.text);
        assert!(out.text.contains("power-saver, balanced"), "{}", out.text);
        assert_eq!(out.exit_code, 2);
    }

    #[test]
    fn profile_set_backend_failure_exits_3_per_safe_004() {
        // SAFE-004: a failed write is reported, never assumed to have
        // succeeded — exit code 3 per cli.md's "underlying interface call
        // failed" convention.
        let provider = FakePowerProfileProvider {
            set_result: Err(ProfileError::BackendFailed("dbus timeout".to_string())),
            ..Default::default()
        };

        let out = run_profile_set(&provider, "balanced");

        assert!(out.text.contains("dbus timeout"), "{}", out.text);
        assert_eq!(out.exit_code, 3);
    }

    #[test]
    fn profile_set_backend_unavailable_exits_3() {
        let provider = FakePowerProfileProvider {
            set_result: Err(ProfileError::BackendUnavailable),
            ..Default::default()
        };

        let out = run_profile_set(&provider, "balanced");

        assert_eq!(out.exit_code, 3);
    }

    #[test]
    fn profile_set_backend_denied_exits_3() {
        let provider = FakePowerProfileProvider {
            set_result: Err(ProfileError::BackendDenied),
            ..Default::default()
        };

        let out = run_profile_set(&provider, "balanced");

        assert_eq!(out.exit_code, 3);
    }
}
