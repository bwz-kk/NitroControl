# NitroControl

Linux-native monitoring and control for Acer Nitro laptops, starting with the
**Acer Nitro V15 (ANV15-41)**. A CLI and a GTK4/libadwaita GUI share one
hardware abstraction; every capability it reports is backed by something
directly verified on real hardware not by feature parity with Acer
NitroSense on Windows, and never by a third-party compatibility table taken
on faith (see [`docs/hardware.md`](docs/hardware.md) for a concrete case
where one was wrong).

## Why

Acer NitroSense is Windows-only. On Linux, this hardware's actual support
surface what's real, what needs an out-of-tree driver, and what simply
doesn't exist wasn't documented anywhere trustworthy. NitroControl is that
documentation made executable: every reading and control this tool exposes
was discovered, verified, and recorded before being wired up, and a
capability with no evidence behind it is reported as `Unsupported`,
`Unknown`, or `RequiresPrivilege` rather than guessed at.

## What it does

**Read-only, no privilege required:**
- CPU / iGPU / dGPU temperature, CPU frequency and utilization, RAM usage
- Battery percentage, charge/discharge status, and power draw
- Fan RPM (explicitly `unavailable` on hardware with no fan `hwmon`
  interface never silently omitted)
- OS power profile (`power-profiles-daemon`, list/get; set needs no
  privilege either verified against PPD's own D-Bus policy)
- `nitroctl diagnose`: the full capability matrix plus the raw sysfs
  path/command evidence behind each reading, safe to paste into a GitHub
  issue (battery serial number and other identifying DMI fields are
  redacted before it's ever printed)

**Gated behind hardware/driver availability, real when present:**
- Acer-firmware power profile (`/sys/firmware/acpi/platform_profile`),
  active when `acer_wmi` is loaded with `predator_v4=1`
- Battery charge limit and calibration mode, via the out-of-tree
  [`bwz-kk/acer-wmi-battery`](https://github.com/bwz-kk/acer-wmi-battery)
  driver — see [`docs/optional-setup.md`](docs/optional-setup.md) before
  opting in; NitroControl never loads or installs either of these itself
- CPU package power draw via RAPL/`powercap` (root-only by default; see
  `docs/optional-setup.md` for the udev-rule relax)

Every one of the above is a real, independently-verified result on this
exact machine, recorded with its evidence in
[`docs/hardware.md`](docs/hardware.md) — not assumed from a spec sheet.

## Install

Requires Rust (stable) and, for the GUI, GTK4 + libadwaita development
packages (`gtk4-devel`/`libgtk-4-dev`, `libadwaita-devel`/`libadwaita-1-dev`
depending on your distro).

```sh
git clone https://github.com/bwz-kk/NitroControl.git
cd NitroControl
cargo build --release
```

Binaries land at `target/release/nitroctl` (CLI) and
`target/release/nitroctl-gui` (GUI). To get the GUI into your app launcher:

```sh
install -Dm755 target/release/nitroctl-gui ~/.local/bin/nitroctl-gui
install -Dm644 nitroctl-gui/data/io.github.nitrocontrol.NitroControl.desktop \
  ~/.local/share/applications/io.github.nitrocontrol.NitroControl.desktop
install -Dm644 nitroctl-gui/data/icons/io.github.nitrocontrol.NitroControl.svg \
  ~/.local/share/icons/hicolor/scalable/apps/io.github.nitrocontrol.NitroControl.svg
```

(Make sure `~/.local/bin` is on your `PATH`.)

## Usage

```sh
nitroctl status              # one-screen summary
nitroctl sensors              # CPU/GPU temps, freq, util, RAM
nitroctl battery               # charge %, status, power draw
nitroctl fans                    # RPM, or "unavailable" if unsupported
nitroctl profile list|get|set <name>
nitroctl acer-profile list|get|set <name>   # needs acer_wmi predator_v4=1
nitroctl battery-limit get|set on|off       # needs acer-wmi-battery
nitroctl battery-calibrate get|set on|off   # needs acer-wmi-battery
nitroctl power-draw            # needs root or a relaxed powercap permission
nitroctl diagnose                # capability matrix + evidence, for bug reports
```

Run `nitroctl-gui` for the graphical dashboard.

## Compatibility

Hardware-provider selection is keyed off `/sys/class/dmi/id/product_name`.
`AcerNitroV15` is the only concrete Acer profile right now; every other
machine falls back to `GenericLinux` (generic `hwmon`/thermal/`power_supply`/
NVML readings only no Acer-specific features). Support for another Acer
model is possible but not claimed until independently verified on that
model see `docs/spec.md`'s `COMPAT-001`/`COMPAT-002`.

## Safety stance

- No raw/arbitrary hardware writes are ever exposed (`nitroctl raw-write`
  does not exist and never will).
- No out-of-tree kernel module is loaded or installed automatically —
  every optional driver in `docs/optional-setup.md` is something you
  choose to set up yourself.
- A failed control write never leaves state ambiguous: NitroControl reports
  the failure and re-reads real hardware state rather than assuming the
  write succeeded.

See [`docs/spec.md`](docs/spec.md)'s Safety Requirements for the full list.

## Project docs

- [`docs/spec.md`](docs/spec.md) functional/non-functional/safety
  requirements
- [`docs/architecture.md`](docs/architecture.md) the provider/capability
  abstraction and why it's shaped the way it is
- [`docs/hardware.md`](docs/hardware.md) the hardware discovery log:
  what was tested, how, and what the evidence actually showed
- [`docs/cli.md`](docs/cli.md) CLI reference
- [`docs/roadmap.md`](docs/roadmap.md) milestone-by-milestone history
- [`docs/optional-setup.md`](docs/optional-setup.md) opt-in driver/udev
  setup for the gated features above

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
