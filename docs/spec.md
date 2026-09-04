# NitroControl — Specification

## 1. Project Goals

NitroControl is a Linux-native monitoring and control utility for Acer Nitro laptops, starting with the **Acer Nitro V15 (ANV15-41)** on CachyOS/Hyprland/Wayland. It exposes hardware telemetry through a stable abstraction, provides a CLI, and (later) a GTK4/libadwaita GUI. Linux/firmware capability, as directly verified on target hardware, is the sole source of truth — not feature parity with Acer NitroSense on Windows.

## 2. Non-Goals

- No feature is implemented because NitroSense has it on Windows; it must be independently verified on Linux.
- No fan control, RGB/keyboard lighting, or battery charge-limit in v1 (M0–M4) — hardware discovery at that time found no supported interface for any of these on this machine. **Update (M5, `roadmap.md`)**: fan RPM read and Acer-firmware power-profile switching were subsequently found to work, real and verified, contingent on `acer_wmi predator_v4=1` — not part of the original v1 scope/acceptance criteria below, tracked separately as FR-007 and the roadmap's M5 items. Direct fan-*speed* control (a `pwm*` write path) remains genuinely unsupported — no such interface was found even with `predator_v4=1` active; only the indirect, profile-driven fan-speed change FR-007 describes exists. **Update (M6, `roadmap.md`)**: battery charge-limit (health mode) also subsequently found to work, contingent on an out-of-tree WMI driver — tracked separately as FR-008, still post-v1, does not gate v1's acceptance criteria below.
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
- **FR-007** (M5, post-v1 addition — see `roadmap.md`): `nitroctl acer-profile list|get|set` operates against `/sys/firmware/acpi/platform_profile` directly (not `power-profiles-daemon`) — a second, independent `PowerProfileProvider` instance from FR-005's, deliberately kept separate rather than merged into `nitroctl profile` (see `architecture.md`'s rationale). Reads (`list`/`get`) require no privilege and report `Unsupported` when the sysfs node is absent (the default — only present when `acer_wmi` is loaded with `predator_v4=1`, which NitroControl never does on its own, per SAFE-001/002). `set` requires write permission on that sysfs path; unlike FR-005's D-Bus policy, the kernel does not grant this to unprivileged users by default, so `set` reports `RequiresPrivilege` unless the user has separately relaxed that permission (e.g. via a udev rule they install themselves — NitroControl does not install one automatically). Real hardware evidence (`hardware.md` M5): 4 of 5 profile values (`low-power`/`quiet`/`balanced`/`balanced-performance`) write successfully and cause a measured fan-speed change; `performance` fails with an EC-level `-EIO` on this hardware, confirmed not fixable by AC power state and confirmed identical on two sibling models — `set_profile("performance")` returns `ProfileError::BackendFailed`, never assumed to have silently succeeded (SAFE-004).

- **FR-008** (M6, post-v1 addition — see `roadmap.md`): `nitroctl battery-limit get|set` operates against `/sys/bus/wmi/drivers/acer-wmi-battery/health_mode`, exposed by the out-of-tree [`bwz-kk/acer-wmi-battery`](https://github.com/bwz-kk/acer-wmi-battery) driver (a fork of `frederik-h/acer-wmi-battery` with an out-of-bounds heap-read fix, `hardware.md`) — **the one exception to this project's standing no-out-of-tree-dependency stance** (`roadmap.md` "Out of scope indefinitely"), made as an explicit, separate M6 decision, not a silent reversal. `set` takes `on`/`off` (a boolean toggle, not a percentage — the driver's `health_mode` caps charging around ~80% when enabled; no finer-grained threshold exists). Reports `Unsupported` when the driver isn't loaded (the default — NitroControl never installs or loads it on its own, same stance as `predator_v4=1`/FR-007). Privilege model for `set` mirrors FR-007 (`RequiresPrivilege` unless the user has separately relaxed the sysfs permission) pending live confirmation of the exact permission bits during implementation. `calibration_mode` is out of scope for FR-008 (a distinct feature, its own future milestone if pursued).

Acceptance: each FR is testable via an automated CLI test (mocked provider) plus one real-hardware run recorded in `hardware.md`. FR-007 additionally requires a recorded real-hardware run of the `performance` failure path, not just the four working values, so the known-bad case has the same evidence standard as the known-good ones. FR-008 requires the same live-binary verification standard as FR-007 (default/unprivileged/privileged states), plus a recorded confirmation that the driver's `dkms status` reports installed/built for the running kernel before any `set` claim.

## 4. Non-Functional Requirements

- **NFR-001**: All read-only functionality in FR-001–004 and FR-006–008 requires no elevated privileges. (FR-005's `set` also needs none, verified empirically in M3 — FR-007's and FR-008's `set` are exceptions, requiring a privilege this project doesn't grant by default; see FR-007/FR-008.)
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
- FR-001 through FR-006 pass both their mocked unit tests and one recorded real-hardware verification each. (FR-007 and FR-008 are M5/M6, post-v1 additions — see `roadmap.md` — each with its own acceptance note above; neither gates v1.)
- No capability is presented as `SUPPORTED` without a corresponding entry in `hardware.md`'s evidence log.
- `nitroctl diagnose` output has been manually reviewed for accidental PII leakage before being documented as a bug-report tool.
- All NFR/SAFE/COMPAT requirements above hold under the test suite described in `docs/cli.md` and the project's test plan.
