//! Generic Linux hardware provider — no Acer-specific claims. Backed only by
//! interfaces confirmed generic in docs/hardware.md: `hwmon`, `/proc/stat`,
//! `/proc/meminfo`, `power_supply`, and `nvidia-smi` for the discrete GPU.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::capability::CapabilityState;
use crate::command::CommandRunner;
use crate::sensor::{
    BatteryState, BatteryStatus, Celsius, GpuKind, Megahertz, MemoryUsage, Percent, Rpm,
    SensorProvider,
};
use crate::sysfs::SysfsReader;

#[derive(Debug, Clone, Copy)]
struct CpuStatSample {
    idle: u64,
    total: u64,
}

pub struct GenericLinux<R: SysfsReader, C: CommandRunner> {
    sysfs: R,
    commands: C,
    cpu_stat_history: Mutex<Option<CpuStatSample>>,
}

impl<R: SysfsReader, C: CommandRunner> GenericLinux<R, C> {
    pub fn new(sysfs: R, commands: C) -> Self {
        Self {
            sysfs,
            commands,
            cpu_stat_history: Mutex::new(None),
        }
    }

    /// Finds the first `hwmon*` directory under `/sys/class/hwmon` whose
    /// `name` file matches one of `chip_names`.
    fn find_hwmon_by_name(&self, chip_names: &[&str]) -> Option<PathBuf> {
        let entries = self.sysfs.read_dir(Path::new("/sys/class/hwmon")).ok()?;
        entries.into_iter().find(|hwmon| {
            self.sysfs
                .read_to_string(&hwmon.join("name"))
                .map(|name| chip_names.contains(&name.trim()))
                .unwrap_or(false)
        })
    }

    /// Reads a millidegree-Celsius sysfs file (e.g. `temp1_input`) into a
    /// `CapabilityState<Celsius>`, distinguishing permission and parse errors.
    fn read_millidegrees(&self, path: &Path) -> CapabilityState<Celsius> {
        // Sane bounds for a laptop CPU/GPU die temperature. Anything outside
        // this range is a corrupt/garbage sensor reading, not real hardware
        // state, and must not be trusted blindly (see roadmap.md M1).
        const PLAUSIBLE_RANGE_C: std::ops::RangeInclusive<f64> = -40.0..=150.0;

        match self.sysfs.read_to_string(path) {
            Ok(raw) => match raw.trim().parse::<f64>() {
                Ok(millidegrees) => {
                    let celsius = millidegrees / 1000.0;
                    if PLAUSIBLE_RANGE_C.contains(&celsius) {
                        CapabilityState::Supported(Celsius(celsius))
                    } else {
                        CapabilityState::Unknown
                    }
                }
                Err(_) => CapabilityState::Unknown,
            },
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                CapabilityState::RequiresPrivilege
            }
            Err(_) => CapabilityState::Unknown,
        }
    }

    fn gpu_temperature_discrete(&self) -> CapabilityState<Celsius> {
        match self.commands.run(
            "nvidia-smi",
            &[
                "--query-gpu=temperature.gpu",
                "--format=csv,noheader,nounits",
            ],
        ) {
            Ok(raw) => match raw.trim().parse::<f64>() {
                Ok(celsius) => CapabilityState::Supported(Celsius(celsius)),
                Err(_) => CapabilityState::Unknown,
            },
            Err(_) => CapabilityState::Unknown,
        }
    }

    fn gpu_utilization_discrete(&self) -> CapabilityState<Percent> {
        match self.commands.run(
            "nvidia-smi",
            &[
                "--query-gpu=utilization.gpu",
                "--format=csv,noheader,nounits",
            ],
        ) {
            Ok(raw) => match raw.trim().parse::<f64>() {
                Ok(percent) => CapabilityState::Supported(Percent(percent)),
                Err(_) => CapabilityState::Unknown,
            },
            Err(_) => CapabilityState::Unknown,
        }
    }
}

fn is_cpu_dir_name(name: &str) -> bool {
    name.strip_prefix("cpu")
        .map(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or(false)
}

fn is_fan_input_name(name: &str) -> bool {
    name.strip_prefix("fan")
        .and_then(|rest| rest.strip_suffix("_input"))
        .map(|index| !index.is_empty() && index.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or(false)
}

fn parse_cpu_stat_line(raw: &str) -> Option<CpuStatSample> {
    let line = raw.lines().find(|l| l.starts_with("cpu "))?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1) // "cpu" label
        .map(|f| f.parse::<u64>())
        .collect::<Result<_, _>>()
        .ok()?;
    // /proc/stat cpu line: user nice system idle iowait irq softirq steal guest guest_nice
    let idle = *fields.get(3)? + fields.get(4).copied().unwrap_or(0);
    let total: u64 = fields.iter().sum();
    Some(CpuStatSample { idle, total })
}

fn parse_meminfo_field(raw: &str, field: &str) -> Option<u64> {
    raw.lines().find_map(|line| {
        let rest = line.strip_prefix(field)?.strip_prefix(':')?;
        rest.split_whitespace().next()?.parse::<u64>().ok()
    })
}

fn parse_uevent(raw: &str) -> std::collections::HashMap<String, String> {
    raw.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

impl<R: SysfsReader, C: CommandRunner> SensorProvider for GenericLinux<R, C> {
    fn cpu_temperature(&self) -> CapabilityState<Celsius> {
        match self.find_hwmon_by_name(&["k10temp", "coretemp"]) {
            Some(hwmon) => self.read_millidegrees(&hwmon.join("temp1_input")),
            None => CapabilityState::Unsupported,
        }
    }

    fn gpu_temperature(&self, gpu: GpuKind) -> CapabilityState<Celsius> {
        match gpu {
            GpuKind::Integrated => match self.find_hwmon_by_name(&["amdgpu"]) {
                Some(hwmon) => self.read_millidegrees(&hwmon.join("temp1_input")),
                None => CapabilityState::Unsupported,
            },
            GpuKind::Discrete => self.gpu_temperature_discrete(),
        }
    }

    fn cpu_utilization(&self) -> CapabilityState<Percent> {
        let raw = match self.sysfs.read_to_string(Path::new("/proc/stat")) {
            Ok(s) => s,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return CapabilityState::Unsupported,
            Err(_) => return CapabilityState::Unknown,
        };
        let Some(sample) = parse_cpu_stat_line(&raw) else {
            return CapabilityState::Unknown;
        };

        let mut history = self.cpu_stat_history.lock().unwrap();
        let result = match *history {
            Some(prev) => {
                let idle_delta = sample.idle.saturating_sub(prev.idle);
                let total_delta = sample.total.saturating_sub(prev.total);
                if total_delta == 0 {
                    CapabilityState::Unknown
                } else {
                    let usage = 100.0 * (1.0 - idle_delta as f64 / total_delta as f64);
                    CapabilityState::Supported(Percent(usage))
                }
            }
            None => CapabilityState::Unknown, // first sample: no baseline to diff against yet
        };
        *history = Some(sample);
        result
    }

    fn gpu_utilization(&self, gpu: GpuKind) -> CapabilityState<Percent> {
        match gpu {
            // Per docs/hardware.md: the amdgpu busy-percent sysfs path was not
            // confirmed on the target machine, so this is explicitly Unknown.
            GpuKind::Integrated => CapabilityState::Unknown,
            GpuKind::Discrete => self.gpu_utilization_discrete(),
        }
    }

    fn cpu_frequency(&self) -> CapabilityState<Megahertz> {
        let entries = match self.sysfs.read_dir(Path::new("/sys/devices/system/cpu")) {
            Ok(e) => e,
            Err(_) => return CapabilityState::Unsupported,
        };
        let cpu_dirs: Vec<_> = entries
            .into_iter()
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(is_cpu_dir_name)
                    .unwrap_or(false)
            })
            .collect();
        if cpu_dirs.is_empty() {
            return CapabilityState::Unsupported;
        }

        let freqs_khz: Vec<f64> = cpu_dirs
            .iter()
            .filter_map(|dir| {
                self.sysfs
                    .read_to_string(&dir.join("cpufreq").join("scaling_cur_freq"))
                    .ok()
                    .and_then(|raw| raw.trim().parse::<f64>().ok())
            })
            .collect();
        if freqs_khz.is_empty() {
            return CapabilityState::Unknown;
        }

        let avg_khz = freqs_khz.iter().sum::<f64>() / freqs_khz.len() as f64;
        CapabilityState::Supported(Megahertz(avg_khz / 1000.0))
    }

    fn ram_usage(&self) -> CapabilityState<MemoryUsage> {
        let raw = match self.sysfs.read_to_string(Path::new("/proc/meminfo")) {
            Ok(s) => s,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return CapabilityState::Unsupported,
            Err(_) => return CapabilityState::Unknown,
        };
        let total_kb = parse_meminfo_field(&raw, "MemTotal");
        let available_kb = parse_meminfo_field(&raw, "MemAvailable");
        match (total_kb, available_kb) {
            (Some(total), Some(available)) if total >= available => {
                CapabilityState::Supported(MemoryUsage {
                    total_bytes: total * 1024,
                    used_bytes: (total - available) * 1024,
                })
            }
            _ => CapabilityState::Unknown,
        }
    }

    fn battery(&self) -> CapabilityState<BatteryState> {
        let entries = match self.sysfs.read_dir(Path::new("/sys/class/power_supply")) {
            Ok(e) => e,
            Err(_) => return CapabilityState::Unsupported,
        };
        let Some(bat_dir) = entries.into_iter().find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("BAT"))
                .unwrap_or(false)
        }) else {
            return CapabilityState::Unsupported;
        };
        let raw = match self.sysfs.read_to_string(&bat_dir.join("uevent")) {
            Ok(s) => s,
            Err(_) => return CapabilityState::Unknown,
        };
        let fields = parse_uevent(&raw);
        let Some(percent) = fields
            .get("POWER_SUPPLY_CAPACITY")
            .and_then(|v| v.parse::<f64>().ok())
        else {
            return CapabilityState::Unknown;
        };
        let status = fields
            .get("POWER_SUPPLY_STATUS")
            .map(|s| BatteryStatus::parse(s))
            .unwrap_or(BatteryStatus::Unknown);
        let power_watts = match (
            fields
                .get("POWER_SUPPLY_VOLTAGE_NOW")
                .and_then(|v| v.parse::<f64>().ok()),
            fields
                .get("POWER_SUPPLY_CURRENT_NOW")
                .and_then(|v| v.parse::<f64>().ok()),
        ) {
            (Some(voltage_uv), Some(current_ua)) => Some((voltage_uv / 1e6) * (current_ua / 1e6)),
            _ => None,
        };

        CapabilityState::Supported(BatteryState {
            percent,
            status,
            power_watts,
        })
    }

    fn fan_rpm(&self) -> CapabilityState<Vec<Rpm>> {
        let hwmon_dirs = match self.sysfs.read_dir(Path::new("/sys/class/hwmon")) {
            Ok(d) => d,
            Err(_) => return CapabilityState::Unsupported,
        };

        let mut rpms = Vec::new();
        for hwmon in hwmon_dirs {
            let Ok(files) = self.sysfs.read_dir(&hwmon) else {
                continue;
            };
            for file in files {
                let is_fan_input = file
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(is_fan_input_name)
                    .unwrap_or(false);
                if !is_fan_input {
                    continue;
                }
                if let Ok(raw) = self.sysfs.read_to_string(&file) {
                    if let Ok(rpm) = raw.trim().parse::<u32>() {
                        rpms.push(Rpm(rpm));
                    }
                }
            }
        }

        if rpms.is_empty() {
            CapabilityState::Unsupported
        } else {
            CapabilityState::Supported(rpms)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::mock::MockCommandRunner;
    use crate::sysfs::mock::MockSysfsReader;

    fn provider(
        sysfs: MockSysfsReader,
        commands: MockCommandRunner,
    ) -> GenericLinux<MockSysfsReader, MockCommandRunner> {
        GenericLinux::new(sysfs, commands)
    }

    fn hwmon_root() -> &'static str {
        "/sys/class/hwmon"
    }

    // ---- cpu_temperature ----

    #[test]
    fn cpu_temperature_reads_k10temp() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_dir(hwmon_root(), vec![PathBuf::from("/sys/class/hwmon/hwmon5")]);
        sysfs.set_content("/sys/class/hwmon/hwmon5/name", "k10temp\n");
        sysfs.set_content("/sys/class/hwmon/hwmon5/temp1_input", "55800\n");
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(
            p.cpu_temperature(),
            CapabilityState::Supported(Celsius(55.8))
        );
    }

    #[test]
    fn cpu_temperature_reads_coretemp_on_intel() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_dir(hwmon_root(), vec![PathBuf::from("/sys/class/hwmon/hwmon2")]);
        sysfs.set_content("/sys/class/hwmon/hwmon2/name", "coretemp\n");
        sysfs.set_content("/sys/class/hwmon/hwmon2/temp1_input", "42000\n");
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(
            p.cpu_temperature(),
            CapabilityState::Supported(Celsius(42.0))
        );
    }

    #[test]
    fn cpu_temperature_unsupported_when_no_matching_chip() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_dir(hwmon_root(), vec![PathBuf::from("/sys/class/hwmon/hwmon0")]);
        sysfs.set_content("/sys/class/hwmon/hwmon0/name", "acpitz\n");
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(p.cpu_temperature(), CapabilityState::Unsupported);
    }

    #[test]
    fn cpu_temperature_unsupported_when_hwmon_class_missing() {
        let sysfs = MockSysfsReader::new(); // /sys/class/hwmon not configured at all
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(p.cpu_temperature(), CapabilityState::Unsupported);
    }

    #[test]
    fn cpu_temperature_requires_privilege_on_permission_denied() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_dir(hwmon_root(), vec![PathBuf::from("/sys/class/hwmon/hwmon5")]);
        sysfs.set_content("/sys/class/hwmon/hwmon5/name", "k10temp\n");
        sysfs.set_permission_denied("/sys/class/hwmon/hwmon5/temp1_input");
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(p.cpu_temperature(), CapabilityState::RequiresPrivilege);
    }

    #[test]
    fn cpu_temperature_unknown_when_implausibly_high() {
        // A sensor reporting 999.9C is a corrupt/garbage reading, not a real
        // temperature — never trusted blindly (roadmap.md M1 boundary case).
        let sysfs = MockSysfsReader::new();
        sysfs.set_dir(hwmon_root(), vec![PathBuf::from("/sys/class/hwmon/hwmon5")]);
        sysfs.set_content("/sys/class/hwmon/hwmon5/name", "k10temp\n");
        sysfs.set_content("/sys/class/hwmon/hwmon5/temp1_input", "999900\n");
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(p.cpu_temperature(), CapabilityState::Unknown);
    }

    #[test]
    fn cpu_temperature_unknown_when_implausibly_negative() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_dir(hwmon_root(), vec![PathBuf::from("/sys/class/hwmon/hwmon5")]);
        sysfs.set_content("/sys/class/hwmon/hwmon5/name", "k10temp\n");
        sysfs.set_content("/sys/class/hwmon/hwmon5/temp1_input", "-500000\n");
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(p.cpu_temperature(), CapabilityState::Unknown);
    }

    #[test]
    fn cpu_temperature_unknown_on_malformed_value() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_dir(hwmon_root(), vec![PathBuf::from("/sys/class/hwmon/hwmon5")]);
        sysfs.set_content("/sys/class/hwmon/hwmon5/name", "k10temp\n");
        sysfs.set_content("/sys/class/hwmon/hwmon5/temp1_input", "not-a-number\n");
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(p.cpu_temperature(), CapabilityState::Unknown);
    }

    // ---- gpu_temperature(Integrated) ----

    #[test]
    fn igpu_temperature_reads_amdgpu_hwmon() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_dir(hwmon_root(), vec![PathBuf::from("/sys/class/hwmon/hwmon4")]);
        sysfs.set_content("/sys/class/hwmon/hwmon4/name", "amdgpu\n");
        sysfs.set_content("/sys/class/hwmon/hwmon4/temp1_input", "53000\n");
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(
            p.gpu_temperature(GpuKind::Integrated),
            CapabilityState::Supported(Celsius(53.0))
        );
    }

    #[test]
    fn igpu_temperature_unsupported_without_amdgpu_hwmon() {
        let sysfs = MockSysfsReader::new();
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(
            p.gpu_temperature(GpuKind::Integrated),
            CapabilityState::Unsupported
        );
    }

    // ---- gpu_temperature(Discrete) / gpu_utilization(Discrete) via nvidia-smi ----

    #[test]
    fn dgpu_temperature_parses_nvidia_smi_output() {
        let commands = MockCommandRunner::new();
        commands.set_output(
            "nvidia-smi",
            &[
                "--query-gpu=temperature.gpu",
                "--format=csv,noheader,nounits",
            ],
            "47\n",
        );
        let p = provider(MockSysfsReader::new(), commands);

        assert_eq!(
            p.gpu_temperature(GpuKind::Discrete),
            CapabilityState::Supported(Celsius(47.0))
        );
    }

    #[test]
    fn dgpu_temperature_unknown_when_nvidia_smi_missing() {
        let p = provider(MockSysfsReader::new(), MockCommandRunner::new());

        assert_eq!(
            p.gpu_temperature(GpuKind::Discrete),
            CapabilityState::Unknown
        );
    }

    #[test]
    fn dgpu_utilization_parses_nvidia_smi_output() {
        let commands = MockCommandRunner::new();
        commands.set_output(
            "nvidia-smi",
            &[
                "--query-gpu=utilization.gpu",
                "--format=csv,noheader,nounits",
            ],
            "12\n",
        );
        let p = provider(MockSysfsReader::new(), commands);

        assert_eq!(
            p.gpu_utilization(GpuKind::Discrete),
            CapabilityState::Supported(Percent(12.0))
        );
    }

    // ---- gpu_utilization(Integrated) ----

    #[test]
    fn igpu_utilization_is_unknown_path_unconfirmed() {
        // Per docs/hardware.md: the amdgpu busy-percent sysfs path wasn't
        // confirmed on this machine, so this is Unknown, not Unsupported.
        let p = provider(MockSysfsReader::new(), MockCommandRunner::new());

        assert_eq!(
            p.gpu_utilization(GpuKind::Integrated),
            CapabilityState::Unknown
        );
    }

    // ---- cpu_utilization ----

    #[test]
    fn cpu_utilization_unknown_on_first_call_no_baseline_yet() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_content("/proc/stat", "cpu  100 0 100 800 0 0 0 0 0 0\n");
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(p.cpu_utilization(), CapabilityState::Unknown);
    }

    #[test]
    fn cpu_utilization_computes_percent_from_second_sample() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_sequence(
            "/proc/stat",
            vec![
                "cpu  100 0 100 800 0 0 0 0 0 0\n".to_string(),
                "cpu  200 0 200 900 0 0 0 0 0 0\n".to_string(),
            ],
        );
        let p = provider(sysfs, MockCommandRunner::new());

        let _ = p.cpu_utilization(); // establishes baseline
                                     // user+system delta = 100+100=200, idle delta = 100, total delta = 300
                                     // usage = 100 * (1 - 100/300) = 66.66...
        match p.cpu_utilization() {
            CapabilityState::Supported(Percent(pct)) => {
                assert!((pct - 66.666).abs() < 0.01, "got {pct}");
            }
            other => panic!("expected Supported, got {other:?}"),
        }
    }

    #[test]
    fn cpu_utilization_never_exceeds_100_percent_even_at_full_load() {
        // Boundary case: zero idle-time delta must saturate at exactly 100%,
        // never overshoot past it (roadmap.md M1 boundary case).
        let sysfs = MockSysfsReader::new();
        sysfs.set_sequence(
            "/proc/stat",
            vec![
                "cpu  100 0 100 500 0 0 0 0 0 0\n".to_string(),
                "cpu  400 0 400 500 0 0 0 0 0 0\n".to_string(),
            ],
        );
        let p = provider(sysfs, MockCommandRunner::new());

        let _ = p.cpu_utilization();
        assert_eq!(
            p.cpu_utilization(),
            CapabilityState::Supported(Percent(100.0))
        );
    }

    #[test]
    fn cpu_utilization_unsupported_when_proc_stat_missing() {
        let p = provider(MockSysfsReader::new(), MockCommandRunner::new());

        assert_eq!(p.cpu_utilization(), CapabilityState::Unsupported);
    }

    #[test]
    fn cpu_utilization_unknown_on_malformed_stat_line() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_content("/proc/stat", "not the stat format\n");
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(p.cpu_utilization(), CapabilityState::Unknown);
    }

    // ---- cpu_frequency ----

    #[test]
    fn cpu_frequency_averages_online_cpus() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_dir(
            "/sys/devices/system/cpu",
            vec![
                PathBuf::from("/sys/devices/system/cpu/cpu0"),
                PathBuf::from("/sys/devices/system/cpu/cpu1"),
                PathBuf::from("/sys/devices/system/cpu/cpufreq"), // not a per-cpu dir, must be ignored
            ],
        );
        sysfs.set_content(
            "/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq",
            "3000000\n",
        );
        sysfs.set_content(
            "/sys/devices/system/cpu/cpu1/cpufreq/scaling_cur_freq",
            "4000000\n",
        );
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(
            p.cpu_frequency(),
            CapabilityState::Supported(Megahertz(3500.0))
        );
    }

    #[test]
    fn cpu_frequency_unsupported_when_no_cpu_dirs_found() {
        let p = provider(MockSysfsReader::new(), MockCommandRunner::new());

        assert_eq!(p.cpu_frequency(), CapabilityState::Unsupported);
    }

    // ---- ram_usage ----

    #[test]
    fn ram_usage_computes_used_from_total_and_available() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_content(
            "/proc/meminfo",
            "MemTotal:       16330000 kB\nMemFree:         2000000 kB\nMemAvailable:   10000000 kB\n",
        );
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(
            p.ram_usage(),
            CapabilityState::Supported(MemoryUsage {
                total_bytes: 16_330_000 * 1024,
                used_bytes: (16_330_000 - 10_000_000) * 1024,
            })
        );
    }

    #[test]
    fn ram_usage_unsupported_when_meminfo_missing() {
        let p = provider(MockSysfsReader::new(), MockCommandRunner::new());

        assert_eq!(p.ram_usage(), CapabilityState::Unsupported);
    }

    #[test]
    fn ram_usage_unknown_when_available_exceeds_total() {
        // MemAvailable > MemTotal is an internally-inconsistent reading —
        // never trusted blindly (roadmap.md M1 boundary case).
        let sysfs = MockSysfsReader::new();
        sysfs.set_content(
            "/proc/meminfo",
            "MemTotal:       1000 kB\nMemAvailable:   2000 kB\n",
        );
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(p.ram_usage(), CapabilityState::Unknown);
    }

    #[test]
    fn ram_usage_unknown_when_fields_missing() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_content("/proc/meminfo", "SomethingElse: 1 kB\n");
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(p.ram_usage(), CapabilityState::Unknown);
    }

    // ---- battery ----

    #[test]
    fn battery_parses_uevent_fields() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_dir(
            "/sys/class/power_supply",
            vec![
                PathBuf::from("/sys/class/power_supply/ACAD"),
                PathBuf::from("/sys/class/power_supply/BAT1"),
            ],
        );
        sysfs.set_content(
            "/sys/class/power_supply/BAT1/uevent",
            "POWER_SUPPLY_STATUS=Discharging\nPOWER_SUPPLY_CAPACITY=87\nPOWER_SUPPLY_VOLTAGE_NOW=17239000\nPOWER_SUPPLY_CURRENT_NOW=2000000\n",
        );
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(
            p.battery(),
            CapabilityState::Supported(BatteryState {
                percent: 87.0,
                status: BatteryStatus::Discharging,
                power_watts: Some(17.239 * 2.0),
            })
        );
    }

    #[test]
    fn battery_unsupported_when_no_battery_present() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_dir(
            "/sys/class/power_supply",
            vec![PathBuf::from("/sys/class/power_supply/ACAD")],
        );
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(p.battery(), CapabilityState::Unsupported);
    }

    #[test]
    fn battery_power_draw_is_none_when_current_missing() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_dir(
            "/sys/class/power_supply",
            vec![PathBuf::from("/sys/class/power_supply/BAT1")],
        );
        sysfs.set_content(
            "/sys/class/power_supply/BAT1/uevent",
            "POWER_SUPPLY_STATUS=Full\nPOWER_SUPPLY_CAPACITY=100\n",
        );
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(
            p.battery(),
            CapabilityState::Supported(BatteryState {
                percent: 100.0,
                status: BatteryStatus::Full,
                power_watts: None,
            })
        );
    }

    // ---- fan_rpm ----

    #[test]
    fn fan_rpm_unsupported_when_no_fan_hwmon_anywhere() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_dir(hwmon_root(), vec![PathBuf::from("/sys/class/hwmon/hwmon5")]);
        sysfs.set_dir(
            "/sys/class/hwmon/hwmon5",
            vec![PathBuf::from("/sys/class/hwmon/hwmon5/temp1_input")],
        );
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(p.fan_rpm(), CapabilityState::Unsupported);
    }

    #[test]
    fn fan_rpm_supported_when_fan_input_present() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_dir(hwmon_root(), vec![PathBuf::from("/sys/class/hwmon/hwmon3")]);
        sysfs.set_dir(
            "/sys/class/hwmon/hwmon3",
            vec![PathBuf::from("/sys/class/hwmon/hwmon3/fan1_input")],
        );
        sysfs.set_content("/sys/class/hwmon/hwmon3/fan1_input", "2400\n");
        let p = provider(sysfs, MockCommandRunner::new());

        assert_eq!(p.fan_rpm(), CapabilityState::Supported(vec![Rpm(2400)]));
    }
}
