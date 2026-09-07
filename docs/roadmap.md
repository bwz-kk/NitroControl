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

## M5 — Fan/thermal control — done 2026-09-04

The `predator_v4=1` experiment was run for real, with the user's explicit consent on each step. **Result: real, working, partial control found.** Full detail and evidence table in `hardware.md`'s "`predator_v4=1` experiment" section; summary:

- Reloading `acer_wmi` with `predator_v4=1` activates a new `hwmon` device (`acer`: `fan1_input`/`fan2_input`/`temp1-3_input`, read-only) and populates `/sys/firmware/acpi/platform_profile` (both entirely absent by default).
- Writing `platform_profile` is the real control surface (no `pwm*` write path exists): `low-power`, `quiet`, `balanced`, `balanced-performance` all write successfully and produce measured, causal fan-speed changes (largest: 2736→4030 RPM going to `balanced-performance`). `performance` fails with an EIO write error. **Root-caused via kernel source**: this is the EC/firmware itself rejecting the "Turbo" tier, not a Linux-side check — retested on AC power at 98% battery (ruling out an AC-gating hypothesis from Acer's own Windows behavior) and still failed identically. Independently confirmed on **two sibling models** (`Div-Acer-Manager-Max` issues #173 ANV15-51, #199 ANV15-52) with the exact same `[Errno 5]` — treated as this hardware class's real ceiling, not a config problem. Full detail in `hardware.md`.
- Fully reversible: module unload/reload without the parameter returned the machine to its exact original state (`hwmon8` gone, `platform_profile` absent again) — confirmed live, not assumed.
- Root-causing the `performance` EIO: done as a follow-up research + live retest pass (see above), treated as this hardware's real ceiling, not pursued further.

**Fan RPM read: done 2026-09-04.** Turned out to need **no `nitroctl-core` code change at all** — `GenericLinux::fan_rpm()` already scans every `hwmon` device generically for `fan*_input` files (not by chip name), so it picked up the `acer` device automatically once `predator_v4=1` was loaded. `AcerNitroV15::fan_rpm()`'s stale comment/test (which pre-dated this discovery and claimed the absence unconditionally) was updated to reflect the real, conditional behavior, plus a regression test locking in the `acer`-hwmon-present case — not a TDD red/green cycle, since nothing was actually broken. Verified end-to-end live: reloaded `acer_wmi predator_v4=1`, ran the real `nitroctl fans` binary, got `Fan RPM: 3032 RPM, 2675 RPM` (exit 0), matched raw sysfs exactly; reloaded without the parameter, confirmed `nitroctl fans` returns to `Fan RPM: unavailable` (exit 1) as before. Module restored to default state, `predator_v4=N`, when done.

**Acer-firmware power-profile design (SPECIFY): done 2026-09-04.** Full design and rationale in `architecture.md`'s new "Acer-firmware power profile (M5, FR-007)" section; FR-007 added to `spec.md`. Summary of the two decisions made, reviewed with the user before writing anything:
- A new `AcerPlatformProfileBackend` implements the *existing* `PowerProfilesBackend` trait against `/sys/firmware/acpi/platform_profile` via `SysfsReader` — reuses `PowerProfilesDaemon<B>`/`PowerProfileProvider`/`ProfileError` unchanged, no new type needed. Rejected the alternative (routing through `power-profiles-daemon`) because it would collapse the 5 real ACPI values down to PPD's 3 and lose the exact granularity M5 proved causes real fan-speed changes.
- Exposed as a separate CLI surface (`nitroctl acer-profile list|get|set`) and a separate read-only GUI row, not merged into FR-005's `nitroctl profile` — genuinely different things (OS-generic vs. Acer-specific/`predator_v4`-gated). `set` reports `RequiresPrivilege` unless the user has separately relaxed that root-owned sysfs file's permission (e.g. a `udev` rule they install themselves) — NitroControl does not install one automatically, same stance as not auto-loading `predator_v4=1` itself.

**Acer-firmware power-profile implementation (FR-007): done 2026-09-04.** TDD throughout, per the design above:
- `SysfsReader` gained its first write method (`write_to_string`) — every other provider was read-only until now. `MockSysfsReader` extended with `set_write_permission_denied`/`set_write_failure`/`last_write_attempt` for testing write paths without touching real hardware.
- `AcerPlatformProfileBackend` implements `PowerProfilesBackend` exactly as designed — `PowerProfilesDaemon`/`PowerProfileProvider`/`ProfileError` all reused unchanged. 18 new `nitroctl-core` tests (backend-level + integration-level through `PowerProfilesDaemon`, including a SAFE-003 invalid-profile-without-writing case and a SAFE-004 failed-write-doesn't-change-reported-state case).
- `nitroctl acer-profile list|get|set` added (`nitroctl-cli`), wording tailored to this backend's real failure modes (names `predator_v4=1` for `Unavailable`, names the root/udev-rule gap for `Denied`) rather than reusing `power-profiles-daemon`'s generic messages. 8 new CLI tests.
- GUI: a second read-only row ("Acer Firmware Profile") in the existing "Power Profile" group, reusing `format::profile_status_row` as-is (already generic over any `CapabilityState<ProfileStatus>`) — no new formatting code needed.
- 129/129 workspace tests pass (18 new in `nitroctl-core`, 8 new in `nitroctl-cli`), clippy/fmt clean.
- **Verified live, end-to-end, through the real `nitroctl` binary** (not just `tee`) — closing the loop the design pass left open:
  - Default state (`predator_v4=N`): `acer-profile list`/`get` → `unavailable` (exit 1); `set quiet` → names `predator_v4=1` in the error (exit 3). Matches design.
  - `predator_v4=1` loaded, unprivileged: `list`/`get` work and match raw sysfs exactly (`low-power, quiet, balanced, balanced-performance, performance` / `balanced`); `set quiet` → denied, names root/udev-rule (exit 3). Matches design.
  - `predator_v4=1` loaded, via `sudo`: `set quiet` → success (exit 0), `get` confirms `quiet` took effect — a real write, through NitroControl's own code, not a manual `tee`. `set performance` → `Input/output error (os error 5)` surfaced verbatim (exit 3, SAFE-004) — the same real EC EIO found in discovery, now reported by `nitroctl` itself. `set balanced` → restored cleanly.
  - Module unloaded/reloaded without the parameter afterward — confirmed back to default (`predator_v4=N`, `platform_profile` absent).
- **User feedback from this session**: `balanced-performance` is the profile they actually want day-to-day on this hardware ("works really well") — the strongest-effect profile confirmed working (2736→4030 RPM in the M5 discovery measurement).

**Persistent config + unprivileged write: done 2026-09-04, applied live on this machine with explicit consent.** Both remaining M5 decisions were settled and actually implemented, not just designed:
- `/etc/modprobe.d/nitrocontrol-acer-wmi.conf` (`options acer_wmi predator_v4=1`) makes `predator_v4=1` load automatically — verified by reloading the module *without* the parameter and confirming it still reads `Y`.
- `/etc/udev/rules.d/90-nitrocontrol-acer-platform-profile.rules` relaxes `/sys/firmware/acpi/platform_profile` to `root:wheel 664` on every module load, using a real udev-matchable class device found this session (`/sys/class/platform-profile/platform-profile-0/`) as the trigger. Verified: `stat` confirms the exact permission, and `nitroctl acer-profile set balanced-performance` / `get` both succeeded **with no `sudo`**.
- Both files documented as a copy-paste, manual, opt-in path for other users in the new `docs/optional-setup.md` — NitroControl-the-program still never installs either one itself, per `architecture.md`'s M5 design stance.
- `balanced-performance` is now the user's working daily-driver profile, applied unprivileged, no manual per-boot reload. (The persistence mechanism applies at every module load unconditionally, including boot, by construction — not separately verified with an actual reboot this session.)

M5 (fan RPM read + Acer-firmware power-profile control, FR-007) is now fully done: discovery, design, implementation, and real-world daily-use setup.

## M6 — Battery charge limit (FR-008) — done 2026-09-04

Roadmap's M5+ battery-limit item resolved: **adopt now**, via a maintained fork, as an explicit exception to the out-of-tree-independence stance (see amended "Out of scope indefinitely" note below) — user's decision, not a default. Full design rationale in `architecture.md`'s new "Battery charge limit (M6, FR-008)" section; FR-008 added to `spec.md`. Summary of the three decisions made, reviewed with the user before writing any `nitroctl-core` code:

- Fork lives at [`bwz-kk/acer-wmi-battery`](https://github.com/bwz-kk/acer-wmi-battery) (from `frederik-h/acer-wmi-battery` `9f90d75`), not vendored into this repo. Carries the M5+ discovery pass's out-of-bounds heap-read fix (`get_battery_health_control_status`/`set_battery_health_control` both dereferenced `obj->buffer.pointer` before validating `obj->buffer.length`) plus a `dkms.conf` upstream didn't ship — pushed 2026-09-04.
- Scoped to `health_mode` only (the actual charge-limit toggle); `calibration_mode` deliberately deferred to its own future milestone.
- Privilege model expected to mirror FR-007 (`RequiresPrivilege` by default, user's own udev rule to relax it) — exact permission bits to be confirmed empirically during implementation, not assumed.

Rejected/deferred: waiting for the in-tree `platform-driver-x86` submission ([LWN #1055804](https://lwn.net/Articles/1055804/)) — still under mailing-list review as of this pass, no ETA, not present in this machine's kernel. Will switch to it (and archive the fork) once/if it merges.

**Implementation (FR-008): done 2026-09-04.** TDD throughout (PR #8, `battery_limit` module in `nitroctl-core`; `nitroctl battery-limit get|set on|off` in `nitroctl-cli`; a read-only GUI row): 18 new tests (8 core, 8 CLI, 2 GUI format), 147/147 workspace tests, clippy/fmt clean.

**Live verification: done 2026-09-04, with explicit user consent.** Fork built (`make LLVM=1`, this machine's kernel), loaded, and driven through the real `nitroctl` binary — default state, unprivileged read, unprivileged write correctly denied, privileged write confirmed via `dmesg` + readback, restore-and-unload confirmed. Full sequence in `hardware.md`'s "M6/FR-008 implementation — live verification" section. Permission bits confirmed live (root-owned, no group/other write) rather than assumed from FR-007's precedent.

**Persistent setup: done 2026-09-04.** DKMS packaging (survives kernel upgrades) + `/etc/modules-load.d` autoload + a udev rule for unprivileged `health_mode` writes, mirroring FR-007's M5 pattern — documented as copy-paste-only in `docs/optional-setup.md`'s new "battery charge limit (M6, FR-008)" section (NitroControl itself never installs any of it, per SAFE-001/002). One real finding along the way: the udev trigger couldn't reuse FR-007's device-class-node pattern — the WMI device itself emits no bind uevent at all, and the module's own `add` event fires *before* the driver's probe creates `health_mode`. The **driver** kobject's `add` event (which the kernel only emits after probe has already run) is the reliable trigger; confirmed live (`root:wheel 664`, unprivileged `nitroctl battery-limit set on` succeeds with exit 0).

M6 is now fully closed out end to end: design, implementation, live verification, and daily-use persistence.

## M7 — Battery calibration mode (FR-009) — scoped 2026-09-04

Roadmap's M6 decision-3 deferral resolved: scoping `calibration_mode` as its own milestone, same driver as M6 (`bwz-kk/acer-wmi-battery`, no new out-of-tree dependency), but a deliberately different risk treatment. Full design rationale in `architecture.md`'s new "Battery calibration mode (M7, FR-009)" section; FR-009 added to `spec.md`. Two decisions made, reviewed with the user before writing any code:

- **Verification is toggle-only** — write→readback→immediate-disable, not a real 12+-hour discharge/recharge cycle. Researched before scoping: the driver's own docs say enabling `calibration_mode` disables `health_mode`, charges to 100%, then does one full discharge and recharge, with no completion signal (the user must notice and disable it manually). Real evidence this is riskier than `health_mode`: the in-tree submission author dropped this exact attribute after it "did not work as expected" on their own test unit ([LWN #1055804](https://lwn.net/Articles/1055804/)). `hardware.md` will record this milestone's evidence standard as explicitly narrower than FR-007/FR-008's — proving the attribute responds, not that a full cycle completes correctly on this hardware.
- **Separate `BatteryCalibrationProvider` trait + `nitroctl battery-calibrate get|set` command**, not folded into M6's `BatteryLimitProvider`/`battery-limit` — same sysfs mechanics, but `set` here starts a many-hour operation with a real side effect (disables `health_mode`) rather than changing a persistent setting, so conflating the two would misrepresent what `set` does.
- **GUI: read-only status row, no write toggle** — a dashboard switch invites an accidental click on a 12+-hour, no-abort-signal operation; `set` stays CLI-only.

**Implementation (FR-009): done 2026-09-04.** TDD throughout (`battery_calibration` module in `nitroctl-core`; `nitroctl battery-calibrate get|set on|off` in `nitroctl-cli`, `set on` carrying an explicit multi-hour-operation caution; a read-only GUI row, no write control): 19 new tests, 166/166 workspace tests, clippy/fmt clean.

**Live verification (toggle-only, per this milestone's narrower evidence standard): done 2026-09-04, with explicit user consent.** Write→dmesg-confirm→readback→immediate-disable through the real `nitroctl` binary, on the already-loaded M6 module (no rebuild needed). Full sequence in `hardware.md`'s "M7/FR-009 implementation" section. Surfaced a real finding not documented in any community write-up consulted during SPECIFY: disabling `calibration_mode` doesn't just stop it — the EC automatically **re-enables `health_mode`** as a side effect (confirmed via `dmesg`, both directions). As designed, this run does **not** establish that a full multi-hour discharge/recharge cycle completes correctly on this hardware — that stays an open gap, same one the in-tree submission's author hit on their own unit.

M7 is now fully closed out to its deliberately narrower scope: design, implementation, and toggle-only live verification. No persistent udev-rule work planned for this one by default — `calibration_mode` stays root-only unless a user separately decides to relax it (not done automatically, same SAFE-001/002 stance).

## M8 — Diagnose evidence-path + redaction (FR-006 gap) — done 2026-09-06

Closed the FR-006 gap M2 deliberately left open (`docs/cli.md`'s M2 note, `spec.md`'s Acceptance Criteria): `diagnose` now emits raw evidence (paths/values), not just each metric's capability state and formatted value.

Two SPECIFY decisions, confirmed with the user via `AskUserQuestion` before writing any code:
- **Scope: FR-001-006 only**, not FR-007/008/009's Acer-specific providers (`acer-profile`/`battery-limit`/`battery-calibrate`) — matches spec.md's Acceptance Criteria wording exactly (only FR-001 through FR-006 gate v1); those FRs have their own acceptance notes with no evidence-path requirement.
- **Evidence = path/command PLUS the actual raw value read, redacted** — not just a bare path name — to satisfy FR-006's literal "raw evidence (paths/values)" wording and its redaction requirement in the same pass, rather than deferring the "values" half again.

Implementation (TDD throughout):
- New `nitroctl-core::evidence` module: `Evidence { source, raw_value }` struct, `redact_evidence()` (suffix-matches `SERIAL_NUMBER`/`_SERIAL`/`_UUID`/`_ASSET_TAG` in `KEY=value` lines, defensive against DMI fields no provider reads yet, not just today's one real case), and a new `EvidenceProvider: SensorProvider` trait — one method per FR-001-006 metric, kept separate from `SensorProvider` itself so `status`/`sensors`/`battery`/`fans` (polled repeatedly by the GUI) don't pay for evidence-string formatting they don't need.
- `GenericLinux`'s internal read helpers (`read_millidegrees`, `nvidia_smi_metric`) were refactored to `_with_raw` variants returning `(CapabilityState<T>, Option<String>)` — a single source of truth both the typed reading and its evidence draw from, so the two can't drift apart on which path was actually checked. `AcerNitroV15` delegates every `EvidenceProvider` method to `GenericLinux`, same pattern as `SensorProvider`.
- `nitroctl-cli`: `run_diagnose` now takes `&dyn EvidenceProvider`; each metric line gets an indented `  evidence: <source> = <raw value>` line when evidence exists (present even for `Unknown`-state garbage readings — useful for a bug report — omitted only for `Unsupported`, nothing to point at). `dmi::build_evidence_provider` added alongside the existing `build_sensor_provider`.
- 25 new tests (17 `GenericLinux` provider tests, 1 `AcerNitroV15` delegation test, 4 `redact_evidence` unit tests, 3 `nitroctl-cli` diagnose tests) — 191/191 workspace tests, clippy/fmt clean.

**Live verification: done 2026-09-06.** Ran the real `nitroctl diagnose` binary on this machine (post-Omarchy-migration, `hardware.md`'s 2026-09-06 update) — every metric line carries a correct evidence line (exact sysfs paths for CPU/iGPU temp, `/proc/stat`/`/proc/meminfo`, hwmon fan paths; the literal `nvidia-smi` command line for dGPU temp/util). Confirmed live: the battery's real `POWER_SUPPLY_SERIAL_NUMBER` field renders as `POWER_SUPPLY_SERIAL_NUMBER=[REDACTED]`, while `POWER_SUPPLY_MODEL_NAME`/`MANUFACTURER`/`CAPACITY` print unredacted (correctly not treated as PII) — grepped the full output for the real serial string, zero matches.

Spec.md's Acceptance Criteria item "`nitroctl diagnose` output has been manually reviewed for accidental PII leakage before being documented as a bug-report tool" is now satisfied for the FR-001-006 evidence this milestone adds.

Also on 2026-09-06: a general-purpose subagent independently audited the whole repo against every M0-M7/FR-001-009 "done" claim in `spec.md`/`roadmap.md`/`architecture.md`/`hardware.md`/`cli.md` before M8 started — no discrepancies found (test counts, trait/CLI-surface existence, and capability-state honesty all matched documented claims exactly), giving a verified-clean baseline to build M8 against.

Before merging, PR #13 was independently validated by two subagents: one confirmed functionality (191/191 tests, clippy/fmt clean, live-hardware evidence cross-checked, no logic bugs — caught one cosmetic test-count arithmetic error in this doc, fixed), the other searched for leaked secrets/PII in the diff (none found).

## M9 — iGPU utilization (`gpu_utilization(Integrated)`) — done 2026-09-06

Closed a M1 discovery-time gap (`hardware.md`'s original note): `gpu_busy_percent`/`pp_dpm_sclk` weren't found at `/sys/class/drm/card1/device/` because `card1` is this machine's NVIDIA dGPU, not the iGPU — the correct card was never disambiguated, so `gpu_utilization(Integrated)` returned a blanket `Unknown` since M1. No new FR needed — this fixes an existing `SensorProvider` method's implementation, not a new capability surface.

Two candidate milestones were found during a fresh DISCOVER pass (also considered: CPU package power draw via `/sys/class/powercap/intel-rapl:0/energy_uj`, deferred — bigger scope, needs a new FR); this one was picked via `AskUserQuestion` for being smaller and closing an existing documented gap rather than adding a new capability.

- `GenericLinux` gained `find_drm_card_by_driver()` — matches `/sys/class/drm/cardN` (filtering out `cardN-<connector>` siblings like `card2-eDP-1`) by `device/uevent`'s `DRIVER` field, since card numbering is boot-arbitrary (confirmed live: `card1`=`nvidia`, `card2`=`amdgpu` on this machine). `read_percent_with_raw()` mirrors `read_millidegrees_with_raw`'s single-source-of-truth pattern for `gpu_busy_percent`-style 0-100 sysfs values.
- `gpu_utilization(Integrated)` now returns `Unsupported` (no amdgpu card found) or a real `Supported`/`Unknown` reading — never the old blanket `Unknown`, which conflated "not checked properly" with "checked, garbage value." `EvidenceProvider::gpu_utilization_evidence(Integrated)` updated to match (was always `None`).
- 5 new tests (2 finding the right card + ignoring connector siblings, 1 unsupported, 1 malformed-value, 1 missing-`/sys/class/drm`; plus 2 evidence tests) — 196/196 workspace tests, clippy/fmt clean.
- **Live-verified**: `nitroctl diagnose`'s iGPU utilization line matched a direct `cat /sys/class/drm/card2/device/gpu_busy_percent` exactly (`0` both ways); `nitroctl sensors`/`status` don't print GPU utilization at all (pre-existing, out of this milestone's scope — only `diagnose` surfaces it today).
- `hardware.md` updated: iGPU utilization's discovery-report bullet and capability-matrix row both changed from `UNKNOWN` to `SUPPORTED`/`Yes`. `pp_dpm_sclk` (iGPU frequency) stays unexamined — no `gpu_frequency` capability exists in `SensorProvider` yet, out of scope here.

## M10 — CPU package power draw (FR-010) — done 2026-09-06

The other candidate M9's DISCOVER pass surfaced (`/sys/class/powercap/intel-rapl:0/energy_uj`, generic RAPL-compatible interface, works despite the "intel" name on this AMD machine), deferred at the time and picked up next via `AskUserQuestion`. Two SPECIFY decisions confirmed with the user before implementing:

- **New separate `PowerDrawProvider` trait**, not folded into `SensorProvider` — matches FR-007/008/009's precedent of adding a new trait per post-v1 capability rather than growing the stable v1 interface.
- **Proceed despite a new finding mid-DISCOVER**: `energy_uj` is root-only (`-r-------- root:root`) on this machine, unlike every other v1 sensor — the first read-only capability that needs privilege by default. Confirmed with the user this was still worth doing (reports `RequiresPrivilege`, matching FR-007/008's stance, with an optional copy-paste udev-rule relax documented in `optional-setup.md` — same as those milestones).

Implementation (TDD throughout):
- New `nitroctl-core::power_draw` module: `PowerDrawProvider` trait (`cpu_package_power() -> CapabilityState<Watts>`), `RaplPowerBackend<R>` backend. `energy_uj` is a monotonically increasing microjoule counter, not an instantaneous reading — average watts derived from two samples over a measured time delta, same statefulness shape as `SensorProvider::cpu_utilization`'s `/proc/stat` rate calc.
- A real finding during implementation: this zone's `max_energy_range_uj` is only ~65.5 J — small enough to wrap within seconds under normal laptop package power, unlike typical Intel RAPL zones (often hundreds of joules). Handled by re-reading `max_energy_range_uj` and adding it back when the counter goes backwards, rather than assuming a fixed range.
- Testability: the rate calculation depends on real elapsed wall-clock time, which isn't itself deterministically mockable — solved by threading an explicit `Instant` parameter through an internal `cpu_package_power_at()` method (the public trait method just calls it with `Instant::now()`), letting tests drive elapsed time via real `Instant + Duration` arithmetic instead of sleeping or introducing a full `Clock` abstraction.
- `nitroctl-cli`: `nitroctl power-draw` (read-only, no `set`) with its own two-sample `sampled_cpu_package_power` wrapper — unlike `cpu_utilization`'s version, it only retries on `Unknown` (no baseline yet), not `RequiresPrivilege`/`Unsupported`, since those are terminal and retrying would add a pointless 200ms to every unprivileged invocation (the common case for this metric). `nitroctl-gui`: read-only "CPU Package Power" row in the CPU group, polled the same way as every other stateful sensor (shared long-lived provider, no artificial sleep needed — successive poll ticks naturally provide the two samples).
- 9 new `nitroctl-core` tests, 3 new `nitroctl-cli` tests, 2 new `nitroctl-gui` format tests — 209/209 workspace tests, clippy/fmt clean.

**Live verification: done 2026-09-06.** Unprivileged `nitroctl power-draw` → `requires elevated privilege`, exit 1 (default state). `sudo nitroctl power-draw` → real reading (`6.4 W`), order-of-magnitude cross-checked against a raw two-sample `cat energy_uj` delta over the same window. The optional udev rule in `optional-setup.md` was also live-tested (not just documented-and-reasoned): applied, confirmed `nitroctl power-draw` works with no `sudo` after (`6.4 W`, exit 0), then reverted — this machine stays at its default root-only state, per SAFE-001/002 (NitroControl doesn't install the relax itself; the user can opt in later by following the doc if they want this as a daily-driver capability, the way `acer-profile`/`battery-limit` were opted into in M5/M6).

## M11 — GUI polish: CPU temperature sparkline — done 2026-09-06

Following a UI-research pass (web search for well-designed GTK4/libadwaita and laptop-control-app UIs — Mission Center, GNOME Resources, asusctl's ROG Control Center, Corsair iCUE/NZXT CAM as references), mocked up one concrete visual upgrade: a live inline history graph on the CPU Temperature row, matching Mission Center/Resources' sparkline convention.

- New `Sparkline` type in `nitroctl-gui::window`: a plain `gtk4::DrawingArea` + Cairo, no charting dependency. Holds a 30-sample ring buffer (1 minute at the existing 2s poll interval), right-aligned so the most recent reading sits at the row's right edge, dynamically scaled to the buffer's own min/max (a fixed-degree fixed range would look flat given how narrow this hardware's real CPU-temp band is), drawn as a filled line in GNOME's default accent blue (`#3584e4`) — legible in both light and dark themes without querying theme state.
- Wired as a suffix widget on the existing "CPU Temperature" `adw::ActionRow` (no new row/group, no layout change) — the row keeps its existing subtitle text, the sparkline sits alongside it.
- `Snapshot` gained a `cpu_temperature_value: Option<f64>` field (only pushed to the sparkline when there's a real value to plot — `Unsupported`/`Unknown`/`RequiresPrivilege` ticks are skipped, not plotted as fabricated points).
- No test regressions (209/209 unaffected — pure UI addition, no `nitroctl-core` change); clippy/fmt clean. Verified via `cargo build`/`cargo test`/`cargo clippy`; a live on-screen check was left to the user (this session's Wayland compositor uses a non-standard Lua-based `hyprctl dispatch` layer that blocked scripted window-focus/screenshot automation — noted as a real friction point, not investigated further this pass).

Only this one row was mocked up, per explicit scope — a template for extending the same `Sparkline` type to other numeric rows (iGPU/dGPU temp, CPU utilization, power draw) later if desired.

## M5+ — remaining re-evaluation items

- **Battery charge limit**: superseded by M6 above — the adoption decision this bullet used to flag as open is now resolved (adopt now, via fork). Kept here only as a pointer for anyone reading roadmap history.
- **Out-of-tree module adoption** (`linuwu_sense`, `facer`, or similar): still out of scope for everything except the one exception M6 makes for `acer-wmi-battery` (see amended "Out of scope indefinitely" note below). Would require NitroControl to solve DKMS packaging and Secure Boot signing itself for any other project, since none of the other surveyed projects ship these by default (`hardware.md` risk).
- **Never trust a third-party compatibility table as evidence** (a community tool lists ANV15-41 as fully supported while this machine's runtime contradicts it) — always re-run Discovery on this exact machine before marking any result `Supported`.

## Out of scope indefinitely (unless hardware evidence changes this)

- Arbitrary/raw hardware writes (`SAFE-002`, permanent).
- Support claims for any Acer model other than ANV15-41 without independent verification on that model (`COMPAT-001`/`COMPAT-002`).
- Forking or depending on any out-of-tree Acer control project (`linuwu_sense`, DAMX, `predator-sense`) for v1 — decision recorded in `hardware.md` §Third-party prior art. **Amended 2026-09-04 (M6)**: `acer-wmi-battery` is a single, explicit, user-reviewed exception (see M6 above) — this bullet's default still holds for every other project and for any future case not separately decided.
