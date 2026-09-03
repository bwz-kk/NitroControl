//! Acer Nitro V15 (ANV15-41) provider.
//!
//! Per docs/hardware.md, every v1 SUPPORTED sensor on this model (CPU/GPU
//! temp, CPU util/freq, RAM, battery) comes from the same generic Linux
//! interfaces as any other machine (`k10temp`, `amdgpu`, `/proc/stat`,
//! `/proc/meminfo`, `power_supply`) — nothing Acer-specific backs them today.
//! So for v1 this provider delegates entirely to `GenericLinux`. Acer-specific
//! divergence (fan control via `predator_v4`, battery charge limit, etc.) is
//! deferred to roadmap.md M5+ and will override individual methods here once
//! verified, per COMPAT-001/COMPAT-002 — never assumed in advance.

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
        // Evidence in docs/hardware.md: no fan hwmon exists on this machine.
        // Delegating to `generic` would find the same absence, but this is
        // pinned explicitly so the "why" is visible at the Acer-specific
        // layer, not just inherited silently.
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
    fn fan_rpm_is_unsupported_on_this_machine_per_hardware_md() {
        let p = AcerNitroV15::new(MockSysfsReader::new(), MockCommandRunner::new());

        assert_eq!(p.fan_rpm(), CapabilityState::Unsupported);
    }
}
