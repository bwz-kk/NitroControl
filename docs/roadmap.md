# NitroControl — Roadmap

Sequence: DISCOVER → SPECIFY → REVIEW → IMPLEMENT → VERIFY → DOCUMENT, one milestone at a time. Each milestone requires implementation + tests + verification + documentation before being called done (see `spec.md` Acceptance Criteria).

## M0 — Specification (this doc set) — done 2026-09-03

`docs/spec.md`, `docs/hardware.md`, `docs/architecture.md`, `docs/cli.md`, `docs/roadmap.md` written from live hardware discovery on the ANV15-41 target machine. No application code, no packages installed, no system config changed.

## M1 — Core read-only providers (`nitroctl-core`)

- Implement `SensorProvider` for `GenericLinux` and `AcerNitroV15` (CPU/GPU temp, CPU freq, CPU util, RAM, battery — per Capability Matrix `SUPPORTED` rows only).
- `fan_rpm()` returns `Unsupported` unconditionally for `AcerNitroV15` on this hardware — no placeholder value.
- Unit tests via the `SysfsReader` seam (`architecture.md`): valid values, malformed values, missing files, permission-denied, boundary values (e.g. negative or absurd temps rejected/flagged, not trusted blindly).
- Verification: cross-check each `SUPPORTED` reading against `sensors`/`nvidia-smi`/`upower` output on the real machine; record the comparison.

## M2 — CLI (`nitroctl-cli`)

- `status`, `sensors`, `battery`, `fans`, `diagnose` per `cli.md`.
- CLI tests run against a mocked `nitroctl-core` provider, covering every exit code path in `cli.md`.
- Verification: run each command for real, compare output to M1's cross-checked values.

## M3 — Power profile control

- Implement `PowerProfileProvider` over `power-profiles-daemon` D-Bus (`zbus`).
- `nitroctl profile list|get|set`.
- Exercise the `set` path end-to-end for the first time (flagged as untested in `hardware.md` risk #5) — verify by reading back `powerprofilesctl get`/D-Bus state after a `set` call.
- Validation: reject any profile name not returned by `list_profiles()`.

## M4 — GUI (`nitroctl-gui`, GTK4 + libadwaita)

- Dashboard: System → CPU, GPU, Memory, Temperatures, Fans, Battery, Power Profile — mirroring `spec.md`'s FR set.
- GUI contains no direct `/sys`/D-Bus/NVML calls — consumes `nitroctl-core` only.
- Unavailable/unsupported states rendered clearly and distinctly from real values (per SAFE/FR-004 conventions).
- Verification: visually compare each displayed value against the CLI's output for the same metric at the same moment.

## M5+ — Re-evaluate currently-unsupported capabilities

Only re-attempt fan control, keyboard backlight, battery charge limit, or Acer firmware thermal profiles if **new evidence** appears (e.g., a BIOS update exposes new ACPI methods, `linuwu_sense` or an equivalent lands in-tree, or a verified per-model interface is found). Any such change starts with a fresh Discovery pass recorded in `hardware.md` — never a code change based on assumption or another model's documentation.

## Out of scope indefinitely (unless hardware evidence changes this)

- Arbitrary/raw hardware writes (`SAFE-002`, permanent).
- Support claims for any Acer model other than ANV15-41 without independent verification on that model (`COMPAT-001`/`COMPAT-002`).
