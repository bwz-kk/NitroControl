# NitroControl — Roadmap

Sequence: DISCOVER → SPECIFY → REVIEW → IMPLEMENT → VERIFY → DOCUMENT, one milestone at a time. Each milestone requires implementation + tests + verification + documentation before being called done (see `spec.md` Acceptance Criteria).

## M0 — Specification (this doc set) — done 2026-09-03

`docs/spec.md`, `docs/hardware.md`, `docs/architecture.md`, `docs/cli.md`, `docs/roadmap.md` written from live hardware discovery on the ANV15-41 target machine. No application code, no packages installed, no system config changed.

## M0.1 — Docs revision from prior-art research — done 2026-09-03

Folded in findings from two web-research passes (Acer-specific ecosystem: `linuwu_sense`, DAMX, `acer-wmi-battery`, `predator-sense`, etc.; Rust architecture: `asusctl`, `system76-power`, `power-profiles-daemon`'s real D-Bus contract, `LenovoLegionLinux`, `fw-fanctrl`) plus a user-supplied lead (a KDE Plasma "Nitro Control" widget referencing `acer_nitro_ec` hwmon and `acer-wmi-battery`'s `health_mode`). Key outcomes: identified the mainline `WMID_GUID4`/`predator_v4` gaming-interface path as the most promising in-tree lead for fan/thermal-profile control (present-but-inactive on this machine); corrected the OS-level power-profile capability to `HardwareDependent` (PPD's placeholder-backend behavior); added `nitroctl-dbus` to the architecture; added SAFE-005 (fail-safe-to-firmware-default); confirmed the decision to stay independent of all surveyed third-party projects for v1. No application code, no packages installed, no system config changed.

## M1 — Core read-only providers (`nitroctl-core`) — done 2026-09-03

- Implemented `SensorProvider` for `GenericLinux` and `AcerNitroV15` (CPU/iGPU/dGPU temp, CPU/dGPU util, CPU freq, RAM, battery, fan RPM), built TDD (red-green per method, 56 tests).
- `fan_rpm()` returns `Unsupported` for both providers on this hardware — no placeholder value; `AcerNitroV15` delegates to `GenericLinux` for every v1 capability (no Acer-specific interface exists yet per `hardware.md`), Acer-specific divergence deferred to M5+.
- Unit tests via the `SysfsReader`/`CommandRunner` seams (`architecture.md`): valid values, malformed values, missing files, permission-denied, and boundary values (implausible temperature readings, internally-inconsistent RAM fields — both map to `Unknown`, never trusted blindly).
- Verification: `examples/verify_m1.rs` ran every reading against real hardware and was cross-checked by hand against `sensors`, `nvidia-smi`, `free`, and `upower` — dGPU temp/util matched `nvidia-smi` exactly, RAM total matched `free` exactly, battery matched `upower`, CPU/iGPU temps within normal sensor-to-sensor drift.
- `cargo test`/`clippy`/`fmt` all clean.

## M2 — CLI (`nitroctl-cli`) — done 2026-09-03

- Implemented `status`, `sensors`, `battery`, `fans`, `diagnose` per `cli.md`, built TDD against a hand-written `FakeProvider` (no filesystem mocking needed — the CLI only depends on `SensorProvider`), 9/9 tests pass.
- `commands.rs` holds pure, testable command logic; `main.rs` is thin clap-based glue with no logic of its own.
- Real-hardware run of every command (`target/debug/nitroctl {status,sensors,battery,fans,diagnose}`) matched M1's cross-checked values; `fans` correctly printed the exact FR-004 text (`Fan RPM: unavailable`) and exited `1`.
- Found and fixed during real-hardware verification (not caught by unit tests, since `FakeProvider` doesn't model statefulness): `cpu_utilization` always read "unknown" because each CLI invocation is a fresh process with no prior `/proc/stat` sample. Fixed with a bounded ~200ms two-sample pause (documented in `cli.md`), re-verified for real.
- A `cavecrew-reviewer` pass over the whole crate found two more coverage gaps (`RequiresPrivilege`/`HardwareDependent` states, and the skip-sleep branch of the CPU-utilization sampling) — fixed with additional tests, 12/12 pass.
- **`diagnose` does not yet fully satisfy FR-006**: it reports each metric's capability state and value, but not the underlying sysfs/NVML/subprocess evidence path FR-006 calls for, since `SensorProvider` doesn't carry that metadata today. Tracked as a known gap (see `cli.md`), not silently claimed as done — closing it needs a `SensorProvider` API extension (e.g. a parallel evidence-path accessor) plus the redaction logic FR-006 already specifies, both deferred to a future milestone rather than expanding M2's scope after the fact.
- `cargo test`/`clippy`/`fmt` all clean.

## M3 — Power profile control — done 2026-09-03

- Implemented `PowerProfileProvider` over `power-profiles-daemon` D-Bus (`zbus::blocking`, no async runtime needed), targeting `org.freedesktop.UPower.PowerProfiles` with fallback probing of legacy `net.hadess.PowerProfiles`. Real `Profiles`/`ActiveProfile` property shape confirmed via `busctl introspect` before writing the parser (`hardware.md`).
- `nitroctl profile list|get|set` implemented and run against real hardware, restoring original state each time; also cross-checked independently via `powerprofilesctl get`.
- `set` end-to-end exercised for the first time (was flagged untested since M1's risk #5) — confirmed working, including the invalid-profile-name rejection path (exit 2, names valid choices) and no-write-attempted-on-invalid-input (SAFE-003).
- `PlatformDriver == "placeholder"` correctly distinguishes a real backend from PPD's placeholder driver; `current_profile()` returns `HardwareDependent(status)` (not a bare unit variant — see below) for `balanced`/`power-saver` on this machine, `Supported(status)` for `performance`.
- **Design fix discovered while building this**: `CapabilityState::HardwareDependent` was a bare unit variant with no payload — would have silently dropped the active profile's name the moment this milestone used it. Changed to `HardwareDependent(T)` (own commit, reviewed as its own logical change) before writing the provider.
- **Privilege-separation decision resolved as moot for M3**: verified live (`powerprofilesctl set` as the normal user, no `sudo`, no polkit prompt) that PPD's own D-Bus policy (`context="default"`) permits unprivileged `set` — corrected the prior `REQUIRES_PRIVILEGE` assumption to `SUPPORTED` in `hardware.md`. NitroControl adds no privilege layer of its own for this milestone; that decision stays open only for a hypothetical future NitroControl-owned privileged daemon (M5+).
- 91/91 workspace tests pass (22 new in `nitroctl-cli`, 12 new + a design-fix regression pass in `nitroctl-core`), clippy/fmt clean.
- Built on branch `m3-power-profiles`, per the new branch+PR workflow.

## M4 — GUI (`nitroctl-gui`, GTK4 + libadwaita) — done 2026-09-03

- Dashboard implemented: `adw::PreferencesPage` grouped into CPU, GPU, Memory, Battery, Fans, Power Profile — mirroring `spec.md`'s FR set. `format.rs` holds pure, TDD'd rendering logic (12 tests); `window.rs` is the only file touching `nitroctl-core`, per architecture.md's layering.
- Unavailable/unsupported/hardware-dependent states rendered as explicit text and styled with the `dim-label` CSS class, distinct from real values (per SAFE/FR-004 conventions) — confirmed visually (Fan RPM correctly shows "unavailable" on this hardware).
- **Design fix found via real verification, not caught by unit tests**: the first working build rebuilt a fresh `SensorProvider`/`PowerProfileProvider` every poll tick (mirroring how the CLI does it once per invocation) — but `GenericLinux::cpu_utilization()`'s rate calculation needs state to persist *across* calls on the same instance, so this meant CPU Utilization read "unknown" forever, on every tick, not just the first one (worse than the CLI's M2 bug, since the GUI polls repeatedly). Screenshotted proof of the bug, then fixed by adding `Send + Sync` supertraits to `SensorProvider`/`PowerProfileProvider` (mirroring `PowerProfilesBackend`'s existing bound) and building both providers **once**, shared via `Arc` across every poll — reconnecting/reconstructing costs are paid once at startup, not per tick. Re-verified with a second screenshot: CPU Utilization showed a real percentage.
- Threading model: every poll's actual sensor/D-Bus reads run via `gio::spawn_blocking` on a worker thread, results applied to widgets via `glib::MainContext::spawn_local` back on the main thread — the GTK main thread is never blocked, per NFR-002. No new async runtime dependency; uses glib's own executor (already transitive via gtk4-rs).
- 105/105 workspace tests pass (12 new in `nitroctl-gui`), clippy/fmt clean.
- Verification: launched the real GUI on the target machine (Hyprland/Wayland), screenshotted it (`grim`, installed for this purpose), and visually compared every displayed value against `nitroctl status`/`sensors` output captured at the same moment — RAM matched exactly, battery matched exactly, temperatures within normal sensor-to-sensor drift, CPU/dGPU utilization matched, iGPU utilization correctly showed "unknown" on both (path unconfirmed per hardware.md).

## M5 — Fan/thermal control — discovery done 2026-09-03, fan RPM read done 2026-09-04

The `predator_v4=1` experiment was run for real, with the user's explicit consent on each step. **Result: real, working, partial control found.** Full detail and evidence table in `hardware.md`'s "`predator_v4=1` experiment" section; summary:

- Reloading `acer_wmi` with `predator_v4=1` activates a new `hwmon` device (`acer`: `fan1_input`/`fan2_input`/`temp1-3_input`, read-only) and populates `/sys/firmware/acpi/platform_profile` (both entirely absent by default).
- Writing `platform_profile` is the real control surface (no `pwm*` write path exists): `low-power`, `quiet`, `balanced`, `balanced-performance` all write successfully and produce measured, causal fan-speed changes (largest: 2736→4030 RPM going to `balanced-performance`). `performance` fails with an EIO write error. **Root-caused via kernel source**: this is the EC/firmware itself rejecting the "Turbo" tier, not a Linux-side check — retested on AC power at 98% battery (ruling out an AC-gating hypothesis from Acer's own Windows behavior) and still failed identically. Independently confirmed on **two sibling models** (`Div-Acer-Manager-Max` issues #173 ANV15-51, #199 ANV15-52) with the exact same `[Errno 5]` — treated as this hardware class's real ceiling, not a config problem. Full detail in `hardware.md`.
- Fully reversible: module unload/reload without the parameter returned the machine to its exact original state (`hwmon8` gone, `platform_profile` absent again) — confirmed live, not assumed.
- Root-causing the `performance` EIO: done as a follow-up research + live retest pass (see above), treated as this hardware's real ceiling, not pursued further.

**Fan RPM read: done 2026-09-04.** Turned out to need **no `nitroctl-core` code change at all** — `GenericLinux::fan_rpm()` already scans every `hwmon` device generically for `fan*_input` files (not by chip name), so it picked up the `acer` device automatically once `predator_v4=1` was loaded. `AcerNitroV15::fan_rpm()`'s stale comment/test (which pre-dated this discovery and claimed the absence unconditionally) was updated to reflect the real, conditional behavior, plus a regression test locking in the `acer`-hwmon-present case — not a TDD red/green cycle, since nothing was actually broken. Verified end-to-end live: reloaded `acer_wmi predator_v4=1`, ran the real `nitroctl fans` binary, got `Fan RPM: 3032 RPM, 2675 RPM` (exit 0), matched raw sysfs exactly; reloaded without the parameter, confirmed `nitroctl fans` returns to `Fan RPM: unavailable` (exit 1) as before. Module restored to default state, `predator_v4=N`, when done.

**Acer-firmware power-profile design (SPECIFY): done 2026-09-04.** Full design and rationale in `architecture.md`'s new "Acer-firmware power profile (M5, FR-007)" section; FR-007 added to `spec.md`. Summary of the two decisions made, reviewed with the user before writing anything:
- A new `AcerPlatformProfileBackend` implements the *existing* `PowerProfilesBackend` trait against `/sys/firmware/acpi/platform_profile` via `SysfsReader` — reuses `PowerProfilesDaemon<B>`/`PowerProfileProvider`/`ProfileError` unchanged, no new type needed. Rejected the alternative (routing through `power-profiles-daemon`) because it would collapse the 5 real ACPI values down to PPD's 3 and lose the exact granularity M5 proved causes real fan-speed changes.
- Exposed as a separate CLI surface (`nitroctl acer-profile list|get|set`) and a separate read-only GUI row, not merged into FR-005's `nitroctl profile` — genuinely different things (OS-generic vs. Acer-specific/`predator_v4`-gated). `set` reports `RequiresPrivilege` unless the user has separately relaxed that root-owned sysfs file's permission (e.g. a `udev` rule they install themselves) — NitroControl does not install one automatically, same stance as not auto-loading `predator_v4=1` itself.

**Remaining M5 work, each still needing its own explicit go-ahead before starting**:
- TDD-implement `AcerPlatformProfileBackend` + the `acer-profile` CLI command + the GUI row, per the design above, behind the usual `SysfsReader` seam, on its own branch+PR per the established workflow.
- Decide & implement a persistent config path (`modprobe.d` boot param) so `predator_v4=1` isn't a manual per-boot reload — a system-configuration change. Current lean (from the design pass above): NitroControl itself stays detect-only, documents this as a manual opt-in step — not yet confirmed as a settled decision.
- Decide whether to document/provide the `udev` rule from the privilege design above as a copy-paste manual step for users who want unprivileged `acer-profile set` — NitroControl still never installs it automatically.

## M5+ — remaining re-evaluation items

- **Battery charge limit**: `acer-wmi-battery`'s platform-driver-x86 mailing-list submission (Jelle van der Waa's v2 series, [LWN 2026-01-25](https://lwn.net/Articles/1055804/)) is still under review, not merged, as of this pass (`hardware.md` Third-party prior art) — keep tracking it; prefer that in-tree interface over any out-of-tree module once/if it lands.
- **Out-of-tree module adoption** (`linuwu_sense`, `acer-wmi-battery`, `facer`, or similar): remains out of scope unless a future decision explicitly revisits it. Would require NitroControl to solve DKMS packaging and Secure Boot signing itself, since none of the surveyed projects ship these by default (`hardware.md` risk).
- **Never trust a third-party compatibility table as evidence** (a community tool lists ANV15-41 as fully supported while this machine's runtime contradicts it) — always re-run Discovery on this exact machine before marking any result `Supported`.

## Out of scope indefinitely (unless hardware evidence changes this)

- Arbitrary/raw hardware writes (`SAFE-002`, permanent).
- Support claims for any Acer model other than ANV15-41 without independent verification on that model (`COMPAT-001`/`COMPAT-002`).
- Forking or depending on any out-of-tree Acer control project (`linuwu_sense`, DAMX, `acer-wmi-battery`, `predator-sense`) for v1 — decision recorded in `hardware.md` §Third-party prior art.
