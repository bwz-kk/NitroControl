# NitroControl — Architecture

## Layering

```
CLI (clap)                 GUI (GTK4 + libadwaita, Milestone 4+)
        \                         /
         Application layer (polling, formatting, CapabilityState)
                        |
         Hardware Abstraction (traits: SensorProvider, PowerProfileProvider)
                        |
   Linux Interfaces (sysfs hwmon/thermal/power_supply, NVML, D-Bus)
                        |
                Kernel / Firmware
```

Hardware-specific logic never appears above the Hardware Abstraction layer. Neither the CLI nor the GUI reads `/sys`, calls `nvidia-smi`/NVML, or touches D-Bus directly — they only call `nitroctl-core` trait methods and render whatever `CapabilityState` comes back.

## Crate layout (Rust workspace)

- **`nitroctl-core`** — the abstraction, providers, and typed sensor/state model. No UI dependency.
- **`nitroctl-cli`** — `clap`-based binary, depends only on `nitroctl-core`.
- **`nitroctl-gui`** — GTK4/libadwaita binary (Milestone 4+), depends only on `nitroctl-core`.

## Core abstraction

```rust
enum CapabilityState<T> {
    Supported(T),
    Unsupported,
    Unknown,
    RequiresPrivilege,
    HardwareDependent,
}

trait SensorProvider {
    fn cpu_temperature(&self) -> CapabilityState<Celsius>;
    fn gpu_temperature(&self, gpu: GpuKind) -> CapabilityState<Celsius>;
    fn cpu_utilization(&self) -> CapabilityState<Percent>;
    fn gpu_utilization(&self, gpu: GpuKind) -> CapabilityState<Percent>;
    fn cpu_frequency(&self) -> CapabilityState<Megahertz>;
    fn ram_usage(&self) -> CapabilityState<MemoryUsage>;
    fn battery(&self) -> CapabilityState<BatteryState>;
    fn fan_rpm(&self) -> CapabilityState<Vec<Rpm>>;
}

trait PowerProfileProvider {
    fn list_profiles(&self) -> CapabilityState<Vec<ProfileName>>;
    fn current_profile(&self) -> CapabilityState<ProfileName>;
    fn set_profile(&self, profile: ProfileName) -> Result<(), ProfileError>;
}
```

`GpuKind` is `Integrated` or `Discrete` — both AMD iGPU and NVIDIA dGPU are modeled explicitly rather than assuming a single GPU, per the discovery findings in `hardware.md`.

`CapabilityState` is never coerced to a bare value in application code — `Unsupported`/`Unknown`/`HardwareDependent` render as explicit text ("unavailable", "unknown") in both CLI and GUI, never as `0` or an empty field, per FR-004/SAFE requirements in `spec.md`.

## Provider selection (compatibility)

At startup, `nitroctl-core` reads `/sys/class/dmi/id/product_name` once:

- `"Nitro ANV15-41"` → `AcerNitroV15` provider (verified capabilities from `hardware.md` only).
- anything else → `GenericLinux` provider (generic `hwmon`/`thermal`/`power_supply`/NVML reads only; makes no Acer-specific claims).

Adding a second Acer model later means adding a new provider struct and a match arm — the CLI/GUI public API (the traits above) does not change, satisfying NFR-004/COMPAT-001.

## Interfaces used, by provider component

| Component | Interface | Crate |
|---|---|---|
| CPU temp/freq/util | `hwmon` (`k10temp`), `/proc/stat`, `cpufreq` sysfs | direct file IO |
| iGPU temp | `hwmon` (`amdgpu`) | direct file IO |
| iGPU util/freq | TBD — path unconfirmed (`hardware.md` risk #1) | direct file IO once confirmed |
| dGPU temp/util/freq/VRAM | NVML | `nvml-wrapper`, fallback to `nvidia-smi` subprocess if NVML binding unavailable |
| RAM | `/proc/meminfo` | direct file IO |
| Battery | `power_supply` (`BAT1`) | direct file IO |
| Fan RPM | none (returns `Unsupported`) | n/a |
| Power profile | `power-profiles-daemon` D-Bus | `zbus` |

## Concurrency / responsiveness (NFR-002)

Sensor polling runs on its own async task (Tokio) or a bounded-interval background thread; CLI commands do a single poll-and-print, the GUI polls on a timer that never blocks the main/UI thread. No sensor read is allowed to block for longer than a short, documented timeout — a timeout is treated as `Unknown`, not a hang.

## Testing seams

Each provider's file-reading logic is isolated behind a small trait (e.g. `SysfsReader`) so unit tests can inject fixture strings for: valid values, malformed values, missing files, and permission-denied errors — without touching the real filesystem. See `docs/roadmap.md` Milestone 1 for the test matrix.
