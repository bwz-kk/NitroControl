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
- `cargo test`/`clippy`/`fmt` all clean.

## M3 — Power profile control

- Implement `PowerProfileProvider` over `power-profiles-daemon` D-Bus (`zbus`), targeting `org.freedesktop.UPower.PowerProfiles` with fallback to legacy `net.hadess.PowerProfiles` (`architecture.md`).
- `nitroctl profile list|get|set`.
- Exercise the `set` path end-to-end for the first time (flagged as untested in `hardware.md` risk #5) — verify by reading back `powerprofilesctl get`/D-Bus state after a `set` call.
- Read the `Profiles` property's driver info to distinguish a real backend from PPD's placeholder driver, mapping the latter to `HardwareDependent` (`hardware.md`).
- Validation: reject any profile name not returned by `list_profiles()`.
- Resolve the polkit-vs-dbus-policy privilege-separation decision (`architecture.md`) before finalizing the daemon's IPC surface.

## M4 — GUI (`nitroctl-gui`, GTK4 + libadwaita)

- Dashboard: System → CPU, GPU, Memory, Temperatures, Fans, Battery, Power Profile — mirroring `spec.md`'s FR set.
- GUI contains no direct `/sys`/D-Bus/NVML calls — consumes `nitroctl-core` only.
- Unavailable/unsupported states rendered clearly and distinctly from real values (per SAFE/FR-004 conventions).
- Verification: visually compare each displayed value against the CLI's output for the same metric at the same moment.

## M5+ — Re-evaluate currently-unsupported capabilities

Prior-art research (M0.1) turned this from an open-ended "wait for new evidence" milestone into a list of concrete, specific next experiments — but each still requires its own explicit user consent before running, since each touches live system/module state:

- **Fan/thermal profile (highest-priority experiment)**: reload `acer_wmi` with `predator_v4=1` (or a matching `force_series` value) and observe whether `platform_profile` or fan-control sysfs surfaces appear. Fully in-tree, reversible by reloading the module without the parameter — no out-of-tree module, no GPL-forking, no Secure Boot signing concern. Try this **before** considering any out-of-tree module. Record the outcome in `hardware.md` regardless of result.
- **Battery charge limit**: track `acer-wmi-battery`'s platform-driver-x86 mailing-list submission for mainline inclusion; if merged, prefer that in-tree interface over any out-of-tree module.
- **Out-of-tree module adoption** (`linuwu_sense`, `acer-wmi-battery`, `facer`, or similar): remains out of scope unless a future decision explicitly revisits it. Would require NitroControl to solve DKMS packaging and Secure Boot signing itself, since none of the surveyed projects ship these by default (`hardware.md` risk).
- **Never trust a third-party compatibility table as evidence** (a community tool lists ANV15-41 as fully supported while this machine's runtime contradicts it) — always re-run Discovery on this exact machine before marking any result `Supported`.

## Out of scope indefinitely (unless hardware evidence changes this)

- Arbitrary/raw hardware writes (`SAFE-002`, permanent).
- Support claims for any Acer model other than ANV15-41 without independent verification on that model (`COMPAT-001`/`COMPAT-002`).
- Forking or depending on any out-of-tree Acer control project (`linuwu_sense`, DAMX, `acer-wmi-battery`, `predator-sense`) for v1 — decision recorded in `hardware.md` §Third-party prior art.
