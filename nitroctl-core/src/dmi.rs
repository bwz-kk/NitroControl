//! Hardware-provider selection, per docs/architecture.md's "Provider selection"
//! section and spec.md's COMPAT-001: reads DMI `product_name` once and picks
//! the matching concrete provider, falling back to `GenericLinux` for any
//! unrecognized machine — never assuming Acer-specific capabilities without
//! a DMI match.

use std::path::Path;

use crate::command::CommandRunner;
use crate::provider::{AcerNitroV15, GenericLinux};
use crate::sensor::SensorProvider;
use crate::sysfs::SysfsReader;

const DMI_PRODUCT_NAME_PATH: &str = "/sys/class/dmi/id/product_name";
const ACER_NITRO_V15_PRODUCT_NAME: &str = "Nitro ANV15-41";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    AcerNitroV15,
    GenericLinux,
}

/// Reads DMI `product_name` and decides which provider matches this machine.
/// A missing or unreadable DMI file is not an error here — it just means we
/// can't identify Acer-specific hardware, so we fall back to `GenericLinux`.
pub fn detect_provider_kind(sysfs: &impl SysfsReader) -> ProviderKind {
    let product_name = sysfs
        .read_to_string(Path::new(DMI_PRODUCT_NAME_PATH))
        .unwrap_or_default();

    if product_name.trim() == ACER_NITRO_V15_PRODUCT_NAME {
        ProviderKind::AcerNitroV15
    } else {
        ProviderKind::GenericLinux
    }
}

/// Builds the `SensorProvider` matching this machine.
pub fn build_sensor_provider<R, C>(sysfs: R, commands: C) -> Box<dyn SensorProvider>
where
    R: SysfsReader + 'static,
    C: CommandRunner + 'static,
{
    match detect_provider_kind(&sysfs) {
        ProviderKind::AcerNitroV15 => Box::new(AcerNitroV15::new(sysfs, commands)),
        ProviderKind::GenericLinux => Box::new(GenericLinux::new(sysfs, commands)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::mock::MockCommandRunner;
    use crate::sysfs::mock::MockSysfsReader;

    #[test]
    fn detects_acer_nitro_v15_by_exact_product_name() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_content(DMI_PRODUCT_NAME_PATH, "Nitro ANV15-41\n");

        assert_eq!(detect_provider_kind(&sysfs), ProviderKind::AcerNitroV15);
    }

    #[test]
    fn falls_back_to_generic_linux_for_unrecognized_product_name() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_content(DMI_PRODUCT_NAME_PATH, "Some Other Laptop\n");

        assert_eq!(detect_provider_kind(&sysfs), ProviderKind::GenericLinux);
    }

    #[test]
    fn falls_back_to_generic_linux_when_dmi_file_missing() {
        let sysfs = MockSysfsReader::new();

        assert_eq!(detect_provider_kind(&sysfs), ProviderKind::GenericLinux);
    }

    #[test]
    fn builds_a_working_provider_for_acer_nitro_v15() {
        let sysfs = MockSysfsReader::new();
        sysfs.set_content(DMI_PRODUCT_NAME_PATH, "Nitro ANV15-41\n");
        sysfs.set_dir(
            "/sys/class/hwmon",
            vec![std::path::PathBuf::from("/sys/class/hwmon/hwmon5")],
        );
        sysfs.set_content("/sys/class/hwmon/hwmon5/name", "k10temp\n");
        sysfs.set_content("/sys/class/hwmon/hwmon5/temp1_input", "42000\n");

        let provider = build_sensor_provider(sysfs, MockCommandRunner::new());

        assert_eq!(
            provider.cpu_temperature(),
            crate::capability::CapabilityState::Supported(crate::sensor::Celsius(42.0))
        );
    }
}
