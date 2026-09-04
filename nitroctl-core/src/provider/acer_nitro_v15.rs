//! Acer Nitro V15 (ANV15-41) provider.
//!
//! Per docs/hardware.md, every v1 SUPPORTED sensor on this model (CPU/GPU
//! temp, CPU util/freq, RAM, battery) comes from the same generic Linux
//! interfaces as any other machine (`k10temp`, `amdgpu`, `/proc/stat`,
//! `/proc/meminfo`, `power_supply`) — nothing Acer-specific backs them today.
//! So for v1 this provider delegates entirely to `GenericLinux`. Acer-specific
//! divergence (battery charge limit, Acer-firmware power profile, etc.) is
//! deferred to roadmap.md M5+ and will override individual methods here once
//! verified, per COMPAT-001/COMPAT-002 — never assumed in advance.
//!
//! **Fan RPM is the one M5 exception already covered, with no code change
//! needed**: `GenericLinux::fan_rpm()` scans every `hwmon` device generically
//! for `fan*_input` files, not by chip name — so when `acer_wmi` is loaded
//! with `predator_v4=1` (see hardware.md's M5 experiment) and its `acer`
//! hwmon device appears with real `fan1_input`/`fan2_input`, delegating here
//! already surfaces it as `Supported`. Verified live in M5 (see roadmap.md).
//! This is not the machine's default boot state — `predator_v4=1` isn't
//! loaded automatically by NitroControl or by CachyOS out of the box, so
//! `fan_rpm()` reads `Unsupported` unless a user has manually enabled it.

use crate::capability::CapabilityState;
use crate::command::CommandRunner;
use crate::provider::generic_linux::GenericLinux;
use crate::sensor::{
    BatteryState, Celsius, GpuKind, Megahertz, MemoryUsage, Percent, Rpm, SensorProvider,
};
use crate::sysfs::SysfsReader;

pub struct AcerNitroV15<R: SysfsReader, C: CommandRunner> {
    generic: GenericLinux<R, C>,
}

impl<R: SysfsReader, C: CommandRunner> AcerNitroV15<R, C> {
    pub fn new(sysfs: R, commands: C) -> Self {
        Self {
            generic: GenericLinux::new(sysfs, commands),
        }
    }
}

impl<R: SysfsReader, C: CommandRunner> SensorProvider for AcerNitroV15<R, C> {
    fn cpu_temperature(&self) -> CapabilityState<Celsius> {
        self.generic.cpu_temperature()
    }

    fn gpu_temperature(&self, gpu: GpuKind) -> CapabilityState<Celsius> {
        self.generic.gpu_temperature(gpu)
    }

    fn cpu_utilization(&self) -> CapabilityState<Percent> {
        self.generic.cpu_utilization()
    }

    fn gpu_utilization(&self, gpu: GpuKind) -> CapabilityState<Percent> {
        self.generic.gpu_utilization(gpu)
    }

    fn cpu_frequency(&self) -> CapabilityState<Megahertz> {
        self.generic.cpu_frequency()
    }

    fn ram_usage(&self) -> CapabilityState<MemoryUsage> {
        self.generic.ram_usage()
    }

    fn battery(&self) -> CapabilityState<BatteryState> {
        self.generic.battery()
    }

    fn fan_rpm(&self) -> CapabilityState<Vec<Rpm>> {
        // See the module doc comment: GenericLinux's generic hwmon scan
        // already picks up the `acer` hwmon device (fan1_input/fan2_input)
        // once predator_v4=1 is loaded — no Acer-specific override needed.
        // Pinned explicitly here (rather than left purely implicit via
        // delegation) so the "why" is visible at the Acer-specific layer.
        self.generic.fan_rpm()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::mock::MockCommandRunner;
    use crate::sysfs::mock::MockSysfsReader;
    use std::path::PathBuf;

    #[test]
    fn delegates_cpu_temperature_to_generic_linux() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_dir(
            "/sys/class/hwmon",
            vec![PathBuf::from("/sys/class/hwmon/hwmon5")],
        );
        sysfs.set_content("/sys/class/hwmon/hwmon5/name", "k10temp\n");
        sysfs.set_content("/sys/class/hwmon/hwmon5/temp1_input", "55800\n");
        let p = AcerNitroV15::new(sysfs, MockCommandRunner::new());

        assert_eq!(
            p.cpu_temperature(),
            CapabilityState::Supported(Celsius(55.8))
        );
    }

    #[test]
    fn fan_rpm_is_unsupported_by_default_predator_v4_not_loaded() {
        let p = AcerNitroV15::new(MockSysfsReader::new(), MockCommandRunner::new());

        assert_eq!(p.fan_rpm(), CapabilityState::Unsupported);
    }

    // Locks in current (already-correct) behavior confirmed live in the M5
    // predator_v4=1 experiment (roadmap.md) — not a red/green TDD cycle,
    // since GenericLinux::fan_rpm()'s generic hwmon scan required no
    // production code change to pick up the `acer` device once present.
    #[test]
    fn fan_rpm_supported_when_acer_hwmon_present_predator_v4_loaded() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_dir(
            "/sys/class/hwmon",
            vec![PathBuf::from("/sys/class/hwmon/hwmon8")],
        );
        sysfs.set_dir(
            "/sys/class/hwmon/hwmon8",
            vec![
                PathBuf::from("/sys/class/hwmon/hwmon8/fan1_input"),
                PathBuf::from("/sys/class/hwmon/hwmon8/fan2_input"),
            ],
        );
        sysfs.set_content("/sys/class/hwmon/hwmon8/name", "acer\n");
        sysfs.set_content("/sys/class/hwmon/hwmon8/fan1_input", "2945\n");
        sysfs.set_content("/sys/class/hwmon/hwmon8/fan2_input", "2576\n");
        let p = AcerNitroV15::new(sysfs, MockCommandRunner::new());

        assert_eq!(
            p.fan_rpm(),
            CapabilityState::Supported(vec![Rpm(2945), Rpm(2576)])
        );
    }
}
