# NitroControl — Architecture

## Layering

```
CLI (clap)                 GUI (GTK4 + libadwaita)
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
- **`nitroctl-gui`** — GTK4/libadwaita binary (M4, done), depends on the same shared layer.

**Resolved for M3** (was an open decision): M3's `profile set` needs no privilege-separation scheme of our own — verified live (`hardware.md`) that `power-profiles-daemon`'s own D-Bus policy is `context="default"` (any user, no polkit) for the whole interface including writes, matching `asusctl`'s model rather than `system76-power`'s. NitroControl is purely a client of PPD's already-permissive bus policy here. The polkit-vs-dbus-policy question is only still open for a **future NitroControl-owned privileged daemon** (M5+ fan/thermal control via `predator_v4`) — that daemon doesn't exist yet, so nothing to decide until then.

## Core abstraction

```rust
enum CapabilityState<T> {
    Supported(T),
    Unsupported,
    Unknown,
    RequiresPrivilege,
    HardwareDependent(T),
}

// Send + Sync: a long-lived instance can be shared (Arc) with a background
// polling thread — the GUI (M4) needs this, since e.g. cpu_utilization()'s
// rate calculation only works across two calls on the *same* instance.
trait SensorProvider: Send + Sync {
    fn cpu_temperature(&self) -> CapabilityState<Celsius>;
    fn gpu_temperature(&self, gpu: GpuKind) -> CapabilityState<Celsius>;
    fn cpu_utilization(&self) -> CapabilityState<Percent>;
    fn gpu_utilization(&self, gpu: GpuKind) -> CapabilityState<Percent>;
    fn cpu_frequency(&self) -> CapabilityState<Megahertz>;
    fn ram_usage(&self) -> CapabilityState<MemoryUsage>;
    fn battery(&self) -> CapabilityState<BatteryState>;
    fn fan_rpm(&self) -> CapabilityState<Vec<Rpm>>;
}

trait PowerProfileProvider: Send + Sync {
    fn list_profiles(&self) -> CapabilityState<Vec<String>>;
    fn current_profile(&self) -> CapabilityState<ProfileStatus>;
    fn set_profile(&self, profile: &str) -> Result<(), ProfileError>;
}
```

`GpuKind` is `Integrated` or `Discrete` — both AMD iGPU and NVIDIA dGPU are modeled explicitly rather than assuming a single GPU, per the discovery findings in `hardware.md`.

`CapabilityState` is never coerced to a bare value in application code — `Unsupported`/`Unknown` render as explicit text ("unavailable", "unknown") in both CLI and GUI, never as `0` or an empty field, per FR-004/SAFE requirements in `spec.md`. `HardwareDependent(T)` carries a real value (unlike `Unsupported`/`Unknown`, which don't) — it means "the interface answered with this value, but the answer may not reflect real hardware behavior," not "no value." A `power-profiles-daemon` profile backed by its "placeholder" driver (D-Bus API present, no real backend underneath — see `hardware.md`) maps to `HardwareDependent(profile_name)`, never bare `Supported`, so the UI still shows which profile is nominally active while flagging that switching it may be a no-op — never silently claiming full, trustworthy hardware control.

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
| Fan RPM | `hwmon` `acer` (`fan*_input`) — only present when `acer_wmi` is loaded with `predator_v4=1` (M5, `hardware.md`); `Unsupported` by default | direct file IO (already covered by `GenericLinux`'s generic hwmon scan, no dedicated code) |
| Power profile (OS-level) | `power-profiles-daemon` D-Bus (`org.freedesktop.UPower.PowerProfiles`, path `/org/freedesktop/UPower/PowerProfiles`, methods `HoldProfile`/`ReleaseProfile`, properties `ActiveProfile`/`Profiles`/`PerformanceDegraded`; probe legacy `net.hadess.PowerProfiles` as fallback) | `zbus` |
| Power profile (Acer-firmware, M5, FR-007) | `/sys/firmware/acpi/platform_profile` + `platform_profile_choices` directly — only present when `acer_wmi` is loaded with `predator_v4=1`; see the design section below | direct file IO (`SysfsReader`) |
| Battery charge limit (M6, FR-008) | `/sys/bus/wmi/drivers/acer-wmi-battery/health_mode` — only present when the out-of-tree `bwz-kk/acer-wmi-battery` module is loaded; see the design section below | direct file IO (`SysfsReader`) |
| Battery calibration mode (M7, FR-009) | `/sys/bus/wmi/drivers/acer-wmi-battery/calibration_mode` — same driver/module as FR-008; see the design section below | direct file IO (`SysfsReader`) |

## Acer-firmware power profile (M5, FR-007) — design (SPECIFY, reviewed with user 2026-09-04)

M5's `predator_v4=1` experiment (`hardware.md`) found a second, independent power-profile control surface — `/sys/firmware/acpi/platform_profile` — distinct from FR-005's `power-profiles-daemon` D-Bus interface. Two real design questions came out of that, both decided with the user before implementation:

**1. A second `PowerProfilesBackend`, not a route through PPD.** Considered and rejected: waiting for `power-profiles-daemon` to adopt `platform_profile` on its own (it prefers that driver when present, but was confirmed live in M5 to *not* pick it up without a service restart) and reusing FR-005's already-unprivileged D-Bus write path. Rejected because PPD's profile set is only 3 values (`performance`/`balanced`/`power-saver`), which would collapse the 5 real ACPI values M5 measured distinct fan behavior for (`low-power` vs `quiet`, `balanced-performance` vs `performance`) — losing exactly the granularity that produced the project's strongest hardware-control signal so far (2736→4030 RPM). `performance` would also almost certainly still `-EIO` through PPD, just with untested error semantics at another layer. Decided instead: a new `AcerPlatformProfileBackend` implementing the *existing* `PowerProfilesBackend` trait (`profiles()`/`active_profile_name()`/`set_active_profile()`) against sysfs via `SysfsReader`, reusing `PowerProfilesDaemon<B>` and `PowerProfileProvider` completely unchanged — this is a second instance of the same abstraction FR-005 already validated, not a new trait or a new `CapabilityState` shape. The `performance`-EIO case fits `ProfileError::BackendFailed(String)` exactly as it already exists; no type-system change needed.

**2. Kept as a separate CLI/GUI surface, not merged into FR-005's.** `nitroctl acer-profile list|get|set` (mirroring `nitroctl profile`'s shape) rather than folding a 5th value into `nitroctl profile`'s 3-profile vocabulary, or auto-selecting one provider over the other. These are genuinely different things — an OS/kernel-generic power-profile concept (FR-005, works on any Linux machine `power-profiles-daemon` supports) versus a raw, Acer-specific, `predator_v4`-gated ACPI surface (FR-007, only real when a user has manually opted in) — conflating them would misrepresent which one a user is actually controlling. GUI: a second read-only `PreferencesGroup` row, matching FR-005's existing GUI treatment (M4's dashboard has no write controls for *either* profile source — `nitroctl profile set`/`nitroctl acer-profile set` stay CLI-only, consistent scope, no new GUI interaction pattern needed).

**3. Privilege**: `/sys/firmware/acpi/platform_profile` is root-owned (`-rw-r--r-- root root`, confirmed live in M5), unlike PPD's D-Bus policy (`context="default"`, verified unprivileged in M3). Reads (`list`/`get`) need no privilege. `set` does. Considered and rejected for v1: NitroControl running as root, or shipping a privileged helper/polkit daemon of its own — both are exactly the "future NitroControl-owned privileged daemon" this project has explicitly deferred since M3 (see the "Resolved for M3" note above), and building one now, for one narrow feature, before any broader need for privilege separation exists, would be scope creep ahead of actual requirements. Decided instead: `set` reports `RequiresPrivilege` (an existing `CapabilityState`/`ProfileError` case, not a new one) unless the user has separately relaxed that file's permission — e.g. a `udev` rule they install themselves. **NitroControl does not install this rule automatically** — like `predator_v4=1` persistence, this is the user's own system-configuration decision, not something `nitroctl` does on its own, per SAFE-001/SAFE-002. **Both steps (persistent `predator_v4=1` and the udev rule) were applied for real on the reference machine in a follow-up session (2026-09-04) — exact file contents, verification steps, and a real udev-matchable device found along the way (`/sys/class/platform-profile/platform-profile-0/`, used purely as the rule's trigger) are in `hardware.md`'s "Persistent `predator_v4=1` + unprivileged write" section and `docs/optional-setup.md`** (a copy-paste guide for other users — still manual, still never run by NitroControl itself).

Implementation (TDD, its own branch+PR) is the next step, not started by this design pass.

## Battery charge limit (M6, FR-008) — design (SPECIFY, reviewed with user 2026-09-04)

M5's battery-limiting research (`hardware.md` "M5+ remaining items") found `frederik-h/acer-wmi-battery`, an out-of-tree WMI driver exposing `health_mode` (a boolean that caps charging around ~80% when enabled). A locally-patched build (fixing a real out-of-bounds heap-read bug found in the process) was tested live in a discovery-only pass with the user's explicit consent — confirmed both reads and writes work on this hardware. Three decisions came out of turning that into a real feature, all reviewed with the user before implementation:

**1. Adopt the out-of-tree driver now, via a maintained fork — the one exception to the standing no-out-of-tree-dependency stance.** Considered and rejected: waiting indefinitely for Jelle van der Waa's in-tree submission ([LWN #1055804](https://lwn.net/Articles/1055804/)) to merge — still under mailing-list review as of this pass, no ETA, and not present in this machine's kernel (`uname -r` `7.2.2-1-cachyos`, module absent). Rejected because the hardware-capability evidence is already solid (M5+ discovery) and the wait has no defined end. Decided instead: adopt now, explicitly revisiting `roadmap.md`'s "Out of scope indefinitely" bullet for this one driver only — not a blanket reversal of the out-of-tree stance, which still holds for `linuwu_sense`/DAMX/`predator-sense` and any other unreviewed project.

**2. The fork lives in its own repo, not vendored into NitroControl.** [`bwz-kk/acer-wmi-battery`](https://github.com/bwz-kk/acer-wmi-battery), forked from `frederik-h/acer-wmi-battery` at `9f90d75`, carries the heap-read fix and a `dkms.conf` upstream doesn't ship. Considered and rejected: vendoring the C source under `contrib/` in this repo — tighter coupling, harder to drop once upstream lands the fix or the in-tree submission merges, and mixes kernel-module C into a Rust workspace's release/PR history. Decided instead: `nitroctl-core` depends on it only as an optional, detected-at-runtime capability (same pattern as `predator_v4=1`) — NitroControl never builds, installs, or loads the module itself; `docs/optional-setup.md` gets a new section for it, same as FR-007's udev rule.

**3. Scope: `health_mode` only, not `calibration_mode`.** The driver also exposes a battery calibration mode (a controlled charge/discharge cycle); both were confirmed working live in the M5+ discovery pass. Decided to scope M6 to `health_mode` (the actual "charge limit" the roadmap has tracked since M0.1) and treat `calibration_mode` as a distinct, later feature if pursued — different risk profile (an active multi-hour hardware cycle vs. a persistent toggle), no reason to couple their release.

Privilege model: expected to mirror FR-007's (`set` reports `RequiresPrivilege` unless the user has separately relaxed the sysfs permission via their own udev rule) — the exact default permission bits on `health_mode` weren't captured during the M5+ discovery pass, so this is confirmed empirically during implementation's live verification, not assumed from FR-007's precedent alone (COMPAT-002).

Implementation (TDD, its own branch+PR) is the next step, not started by this design pass.

## Battery calibration mode (M7, FR-009) — design (SPECIFY, reviewed with user 2026-09-04)

M6 deliberately scoped `calibration_mode` out (`architecture.md`'s M6 section, decision 3). This section covers turning it into its own real feature. Two decisions, both reviewed with the user before implementation:

**1. Verification is toggle-only, not a real calibration cycle.** Researched before scoping (not assumed): the driver's own docs and community reports say enabling `calibration_mode` disables `health_mode`, charges to 100%, then does one full discharge and one full recharge — 12+ hours, and the driver never signals completion (the user has to notice and disable it manually). The in-tree submission's author dropped this exact attribute after it "did not work as expected" on their own test unit ([LWN #1055804](https://lwn.net/Articles/1055804/)) — real, dated evidence this feature is less reliable than `health_mode`, not a hypothetical concern. Considered and rejected: running a full real cycle on the reference machine to get the same evidence standard as FR-007/FR-008. Rejected because the cost (12+ hours unusable, uncertain outcome, no abort/completion signal from the driver) is disproportionate to what M7 needs to prove — that the sysfs attribute exists and responds. Decided instead: live verification for this milestone is a write→readback→immediate-disable round trip only (mirrors the mechanics FR-008 already verified for `health_mode`, at the same attribute-level granularity) — `hardware.md` records this as an explicit, narrower evidence standard for FR-009, not a `Supported` claim about the full discharge/recharge cycle. If a user ever runs a full cycle and reports back, that's new evidence for a future pass, not assumed now.

**2. A separate `BatteryCalibrationProvider` trait and CLI command, not a second method on `BatteryLimitProvider`.** Considered and rejected: adding `calibration_mode()`/`set_calibration_mode()` to the existing M6 trait, since the sysfs mechanics are identical (same driver, same `DRIVER_ATTR_RW` pattern, same boolean write). Rejected because the *meaning* of `set` is fundamentally different — `BatteryLimitProvider::set_health_mode` changes a persistent charge-cap setting with no ongoing side effect once written, while calibration's `set(true)` kicks off a many-hour hardware operation that actively disables `health_mode` as a side effect and leaves the driver in a state the user must remember to undo. Folding both under one trait/command would let a caller treat calibration as casually as a charge-cap toggle, which it isn't. Decided instead: new `BatteryCalibrationProvider` trait (`calibration_mode()`/`set_calibration_mode()`, its own `CalibrationError` mirroring `BatteryLimitError`'s shape) and a separate `nitroctl battery-calibrate get|set` command, matching FR-007/FR-008's precedent of keeping semantically distinct controls on distinct command surfaces rather than merging them for surface-area convenience.

**GUI**: read-only status row only (mirrors FR-005/FR-007's existing GUI treatment — no write controls in the dashboard for those either), deliberately **no write toggle** in the GUI for this one. A dashboard switch invites a casual click; a 12+-hour, no-abort-signal, health-mode-disabling operation shouldn't be one accidental click away. `nitroctl battery-calibrate set` stays CLI-only, same as every other `set` in this project.

Privilege model mirrors FR-008 (`RequiresPrivilege` unless the user has separately relaxed the sysfs permission) — confirmed empirically during implementation, not assumed.

Implementation (TDD, its own branch+PR) is the next step, not started by this design pass.

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
- **[LACT](https://github.com/ilya-zlobintsev/LACT)** (Rust) — GPU fan-curve/clock/power-limit control for AMD (full) and NVIDIA (via NVML, since v0.6.0). Confirms the `asusctl`-style daemon+socket+systemd architecture at a second, independent project: `lactd` runs as root behind a Unix socket gated by group/user config, no polkit. Also confirms this project's own hwmon finding independently — LACT's own docs warn fan control may be unavailable "on laptops where the fan isn't wired through the GPU," same failure mode as this machine's absent fan hwmon. **Explicitly a weaker SAFE-005 example, not a stronger one**: LACT has a provisional-apply/confirm timer (a bad config auto-reverts after 5s) but no watchdog restoring safe state if `lactd` itself crashes mid-curve (open, unresolved: [issue #359](https://github.com/ilya-zlobintsev/LACT/issues/359)) — `fw-fanctrl` remains the pattern to copy for SAFE-005, LACT is cited here only for the daemon/socket/systemd shape.
