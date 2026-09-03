# NitroControl — Architecture

## Layering

```
CLI (clap)                 GUI (GTK4 + libadwaita, Milestone 4+)
        \                         /
         nitroctl-dbus (shared capability/state proxy)
                        |
         Application layer (polling, formatting, CapabilityState)
                        |
         Hardware Abstraction (traits: SensorProvider, PowerProfileProvider)
                        |
   Linux Interfaces (sysfs hwmon/thermal/power_supply, NVML, D-Bus)
                        |
                Kernel / Firmware
```

Hardware-specific logic never appears above the Hardware Abstraction layer. Neither the CLI nor the GUI reads `/sys`, calls `nvidia-smi`/NVML, or touches D-Bus directly — they only call `nitroctl-core` trait methods (via `nitroctl-dbus`) and render whatever `CapabilityState` comes back.

## Crate layout (Rust workspace)

- **`nitroctl-core`** — the abstraction, providers, and typed sensor/state model. No UI dependency.
- **`nitroctl-dbus`** — shared proxy crate exposing `CapabilityState` over D-Bus to both frontends, so the CLI and GUI never re-derive support state independently. Pattern taken from `asusctl`'s `rog-dbus` crate (prior art, see below).
- **`nitroctl-cli`** — `clap`-based binary, depends on `nitroctl-core` (directly, or via `nitroctl-dbus` once the daemon exists).
- **`nitroctl-gui`** — GTK4/libadwaita binary (Milestone 4+), depends on the same shared layer.

**Open decision for M3**: privilege-separation model for any write `nitroctl-dbus` exposes. Prior art splits two ways — `asusctl` relies on plain D-Bus system-bus policy files (coarse UID/group allow-list), `system76-power` uses polkit (per-action, granular). `RequiresPrivilege` in our `CapabilityState` enum implies per-action granularity, which argues for **polkit** — confirm this before building the daemon's IPC surface, since retrofitting later touches every method signature.

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

`CapabilityState` is never coerced to a bare value in application code — `Unsupported`/`Unknown`/`HardwareDependent` render as explicit text ("unavailable", "unknown") in both CLI and GUI, never as `0` or an empty field, per FR-004/SAFE requirements in `spec.md`. A `power-profiles-daemon` profile backed by its "placeholder" driver (D-Bus API present, no real backend underneath — see `hardware.md`) maps to `HardwareDependent`, never `Supported`, so the UI doesn't claim real hardware control that isn't happening.

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
| Fan RPM | none active (returns `Unsupported`) | n/a |
| Power profile | `power-profiles-daemon` D-Bus (`org.freedesktop.UPower.PowerProfiles`, path `/org/freedesktop/UPower/PowerProfiles`, methods `HoldProfile`/`ReleaseProfile`, properties `ActiveProfile`/`Profiles`/`PerformanceDegraded`; probe legacy `net.hadess.PowerProfiles` as fallback) | `zbus` |

## Concurrency / responsiveness (NFR-002)

Sensor polling runs on its own async task (Tokio) or a bounded-interval background thread; CLI commands do a single poll-and-print, the GUI polls on a timer that never blocks the main/UI thread. No sensor read is allowed to block for longer than a short, documented timeout — a timeout is treated as `Unknown`, not a hang.

## Testing seams

Each provider's file-reading logic is isolated behind a small trait (e.g. `SysfsReader`) so unit tests can inject fixture strings for: valid values, malformed values, missing files, and permission-denied errors — without touching the real filesystem. See `docs/roadmap.md` Milestone 1 for the test matrix.

## Fail-safe on daemon stop (SAFE-005)

Any future control feature (fan curve, keyboard lighting, etc.) that actively holds a hardware state while the daemon runs must have a defined "return to firmware default" path invoked on clean shutdown, `SIGTERM`, and panic — never leave hardware frozen at the last-commanded value. Adopted from the `fw-fanctrl` project's design (Framework laptops): stopping their service returns fans to firmware's own default curve rather than any software-remembered value. Not yet applicable to v1 (no control features implemented), but the daemon's shutdown path should be built with this hook from the start so it isn't retrofitted later.

## Prior art consulted

- **[asusctl](https://gitlab.com/asus-linux/asusctl)** (MPL-2.0, Rust) — closest structural analog; source of the `nitroctl-dbus`/`rog-dbus` pattern and the capability-query-then-render UI convention.
- **[system76-power](https://github.com/pop-os/system76-power)** (GPL-3.0, Rust) — considered and rejected alternative of *replacing* `power-profiles-daemon` by emulating its D-Bus interface; source of the polkit privilege-separation model we're leaning toward.
- **[power-profiles-daemon](https://gitlab.freedesktop.org/upower/power-profiles-daemon)** (upstream) — source of the real D-Bus contract used in FR-005/M3, and of the "placeholder backend" behavior now reflected in our `CapabilityState` mapping.
- **[LenovoLegionLinux](https://github.com/johnfanv2/LenovoLegionLinux)** — source of the dual-threshold-hysteresis + minimum-PWM-floor fan-curve design to reuse if fan-curve control is ever implemented (M5+).
- **[fw-fanctrl](https://github.com/TamtamHero/fw-fanctrl)** — source of SAFE-005's fail-safe-to-firmware-default pattern.
