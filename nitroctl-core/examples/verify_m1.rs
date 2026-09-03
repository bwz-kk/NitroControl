//! Real-hardware verification for M1, per docs/roadmap.md: prints every
//! `SensorProvider` reading from the real machine so it can be cross-checked
//! against `sensors`, `nvidia-smi`, `upower`, and `free` output by hand.
//! Not a test — a manual verification aid. Run with:
//!   cargo run -p nitroctl-core --example verify_m1

use nitroctl_core::command::RealCommandRunner;
use nitroctl_core::dmi;
use nitroctl_core::sensor::GpuKind;
use nitroctl_core::sysfs::RealSysfsReader;

fn main() {
    let provider = dmi::build_sensor_provider(RealSysfsReader, RealCommandRunner);

    println!(
        "cpu_temperature:            {:?}",
        provider.cpu_temperature()
    );
    println!(
        "gpu_temperature(Integrated): {:?}",
        provider.gpu_temperature(GpuKind::Integrated)
    );
    println!(
        "gpu_temperature(Discrete):   {:?}",
        provider.gpu_temperature(GpuKind::Discrete)
    );
    println!("cpu_frequency:              {:?}", provider.cpu_frequency());
    println!("ram_usage:                  {:?}", provider.ram_usage());
    println!("battery:                    {:?}", provider.battery());
    println!("fan_rpm:                    {:?}", provider.fan_rpm());
    println!(
        "gpu_utilization(Integrated): {:?}",
        provider.gpu_utilization(GpuKind::Integrated)
    );
    println!(
        "gpu_utilization(Discrete):   {:?}",
        provider.gpu_utilization(GpuKind::Discrete)
    );

    // cpu_utilization needs two samples; take a second after a short pause.
    let _ = provider.cpu_utilization();
    std::thread::sleep(std::time::Duration::from_millis(300));
    println!(
        "cpu_utilization:            {:?}",
        provider.cpu_utilization()
    );
}
