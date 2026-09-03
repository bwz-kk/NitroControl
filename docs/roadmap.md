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

## M4 — GUI (`nitroctl-gui`, GTK4 + libadwaita)

- Dashboard: System → CPU, GPU, Memory, Temperatures, Fans, Battery, Power Profile — mirroring `spec.md`'s FR set.
- GUI contains no direct `/sys`/D-Bus/NVML calls — consumes `nitroctl-core` only.
- Unavailable/unsupported states rendered clearly and distinctly from real values (per SAFE/FR-004 conventions).
- Verification: visually compare each displayed value against the CLI's output for the same metric at the same moment.

## M5+ — Re-evaluate currently-unsupported capabilities

Prior-art research (M0.1) turned this from an open-ended "wait for new evidence" milestone into a list of concrete, specific next experiments — but each still requires its own explicit user consent before running, since each touches live system/module state:

- **Fan/thermal profile (highest-priority experiment)**: reload `acer_wmi` with `predator_v4=1` (or a matching `force_series` value) and re-scan `/sys/class/hwmon` for new `pwm*`/`fan*_input` files, and `/sys/firmware/acpi/platform_profile` for activation — per `luizjr/nitro-sense-linux` (hardware.md), a working `predator_v4` model exposes fan control there directly, not through a bespoke API. Fully in-tree, reversible by reloading the module without the parameter — no out-of-tree module, no GPL-forking, no Secure Boot signing concern. Try this **before** considering any out-of-tree module. Record the outcome in `hardware.md` regardless of result.
- **Battery charge limit**: track `acer-wmi-battery`'s platform-driver-x86 mailing-list submission for mainline inclusion; if merged, prefer that in-tree interface over any out-of-tree module.
- **Out-of-tree module adoption** (`linuwu_sense`, `acer-wmi-battery`, `facer`, or similar): remains out of scope unless a future decision explicitly revisits it. Would require NitroControl to solve DKMS packaging and Secure Boot signing itself, since none of the surveyed projects ship these by default (`hardware.md` risk).
- **Never trust a third-party compatibility table as evidence** (a community tool lists ANV15-41 as fully supported while this machine's runtime contradicts it) — always re-run Discovery on this exact machine before marking any result `Supported`.

## Out of scope indefinitely (unless hardware evidence changes this)

- Arbitrary/raw hardware writes (`SAFE-002`, permanent).
- Support claims for any Acer model other than ANV15-41 without independent verification on that model (`COMPAT-001`/`COMPAT-002`).
- Forking or depending on any out-of-tree Acer control project (`linuwu_sense`, DAMX, `acer-wmi-battery`, `predator-sense`) for v1 — decision recorded in `hardware.md` §Third-party prior art.
