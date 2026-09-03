# NitroControl — Specification

## 1. Project Goals

NitroControl is a Linux-native monitoring and control utility for Acer Nitro laptops, starting with the **Acer Nitro V15 (ANV15-41)** on CachyOS/Hyprland/Wayland. It exposes hardware telemetry through a stable abstraction, provides a CLI, and (later) a GTK4/libadwaita GUI. Linux/firmware capability, as directly verified on target hardware, is the sole source of truth — not feature parity with Acer NitroSense on Windows.

## 2. Non-Goals

- No feature is implemented because NitroSense has it on Windows; it must be independently verified on Linux.
- No fan control, RGB/keyboard lighting, battery charge-limit, or Acer firmware thermal-profile switching in v1 — current hardware discovery (see `hardware.md`) found no supported interface for any of these on this machine.
- No raw/arbitrary hardware writes are ever exposed to the user.
- No modification of firmware or kernel parameters.
- No claim of support for any Acer model other than ANV15-41 until independently tested.

## 3. Functional Requirements

- **FR-001**: `nitroctl sensors` reports CPU temperature, iGPU temperature, dGPU temperature, CPU frequency, CPU utilization, and RAM usage, each sourced from a verified interface (see `hardware.md` §Capability Matrix).
- **FR-002**: `nitroctl status` gives a one-screen summary combining FR-001's metrics with battery state and current OS power profile.
- **FR-003**: `nitroctl battery` reports charge percentage, charging/discharging/full status, and power draw (W = V×I) when derivable from `BAT1`. Unavailable fields render as `unavailable`, never a fabricated `0`.
- **FR-004**: `nitroctl fans` reports `unavailable` explicitly — evidence shows no fan `hwmon` interface exists on this machine — rather than omitting the command or returning a fake value.
- **FR-005**: `nitroctl profile list|get|set` operates against `power-profiles-daemon` over D-Bus — bus/interface `org.freedesktop.UPower.PowerProfiles` at `/org/freedesktop/UPower/PowerProfiles`, falling back to the legacy `net.hadess.PowerProfiles` name/path if the former isn't registered (`performance`/`balanced`/`power-saver`). Verified empirically (M3, `hardware.md`): PPD's own D-Bus policy allows any user to call `set` with no polkit prompt — NitroControl relies on that existing policy as-is and introduces no privilege layer of its own. A profile backed by PPD's "placeholder" driver (see `hardware.md`) is reported as `HardwareDependent`, not `Supported`.
- **FR-006**: `nitroctl diagnose` emits the capability matrix plus the raw evidence (paths/values) NitroControl used to derive it, suitable for pasting into a GitHub issue, with battery serial number and identifying DMI fields redacted.

Acceptance: each FR is testable via an automated CLI test (mocked provider) plus one real-hardware run recorded in `hardware.md`.

## 4. Non-Functional Requirements

- **NFR-001**: All read-only functionality in FR-001–004 requires no elevated privileges.
- **NFR-002**: Telemetry polling never blocks CLI output or (later) GUI rendering — bounded-latency or async polling only.
- **NFR-003**: The core hardware abstraction has no UI dependency; CLI and GUI both consume the same `nitroctl-core` API.
- **NFR-004**: Adding a new hardware provider (e.g. a second Acer model) must not require changes to the CLI or GUI's public interface.

## 5. Safety Requirements

- **SAFE-001**: NitroControl never writes to a sysfs/WMI/EC path that hasn't been enumerated and verified in `hardware.md`.
- **SAFE-002**: No command exposes arbitrary raw reads/writes to a hardware path (no `nitroctl raw-write <path> <value>`).
- **SAFE-003**: All control-feature inputs are validated against a known-valid range/enum before any write is attempted; invalid input is rejected with a clear error, never clamped silently.
- **SAFE-004**: A failed control write leaves hardware state unchanged from NitroControl's perspective — no partial-write recovery logic that could leave state ambiguous; NitroControl reports the failure and re-reads actual state rather than assuming success.
- **SAFE-005**: On daemon stop or crash, any control feature that was actively holding hardware state releases back to firmware/EC default behavior — the daemon never leaves hardware frozen at its last-commanded value (pattern confirmed from the `fw-fanctrl` project's design for Framework laptops).

## 6. Compatibility Requirements

- **COMPAT-001**: Hardware-provider selection is keyed off `/sys/class/dmi/id/product_name`, read once at startup. `AcerNitroV15` is the only concrete Acer profile in v1; `GenericLinux` (generic hwmon/thermal/power_supply/NVML only) is the fallback for any unrecognized machine.
- **COMPAT-002**: A capability is only ever marked `SUPPORTED` in code after being verified on real hardware and recorded in `hardware.md`; otherwise it is `UNSUPPORTED`, `UNKNOWN`, `REQUIRES_PRIVILEGE`, or `HARDWARE_DEPENDENT`. A third-party project's own compatibility claim is never sufficient evidence on its own — confirmed necessary by a concrete case in `hardware.md` (a community tool lists ANV15-41 as fully supported while this machine's own runtime state contradicts it).

## 7. Acceptance Criteria

A v1 release is acceptable when:
- FR-001 through FR-006 pass both their mocked unit tests and one recorded real-hardware verification each.
- No capability is presented as `SUPPORTED` without a corresponding entry in `hardware.md`'s evidence log.
- `nitroctl diagnose` output has been manually reviewed for accidental PII leakage before being documented as a bug-report tool.
- All NFR/SAFE/COMPAT requirements above hold under the test suite described in `docs/cli.md` and the project's test plan.
