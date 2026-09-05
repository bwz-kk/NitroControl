# NitroControl — Hardware Discovery Report

Target device: **Acer Nitro V15 (ANV15-41)**, BIOS V1.20 (10/23/2025), board `Sportage_RBH`.
OS: CachyOS, kernel `7.2.2-1-cachyos`, Hyprland/Wayland.

All findings below were gathered by direct, read-only inspection of this exact machine (`/sys`, `/proc`, `journalctl`, `lsmod`, `modinfo`, `nvidia-smi`, `sensors`, `systemctl`) on 2026-09-03. Nothing here is inferred from vendor documentation or other models unless explicitly labeled "documented, not verified."

## Identity

```
$ cat /sys/class/dmi/id/product_name  → Nitro ANV15-41
$ cat /sys/class/dmi/id/product_version → V1.20
$ cat /sys/class/dmi/id/sys_vendor → Acer
$ cat /sys/class/dmi/id/board_name → Sportage_RBH
```

## CPU — AMD Ryzen 7 7735HS

- Temperature: `hwmon5` name `k10temp`, `temp1_input`/`temp1_label=Tctl`. Cross-checked against `sensors` (`Tctl: +55.8°C`).
- Frequency/governor: `/sys/devices/system/cpu/cpu*/cpufreq/` fully populated — `scaling_cur_freq`, `scaling_governor` (`performance` observed), `scaling_driver=amd-pstate-epp`, `energy_performance_preference` + `energy_performance_available_preferences`, `boost`.
- Utilization: standard `/proc/stat` deltas (no special interface needed).

## GPU — dual (AMD iGPU + NVIDIA dGPU)

### AMD iGPU (Radeon, integrated in the 7735HS)
- `hwmon4` name `amdgpu`: `temp1_input`/`temp1_label`, `power1_input`/`power1_label`, `in0`/`in1` voltage rails (`vddgfx`, `vddnb` per `sensors`).
- Utilization/frequency (`gpu_busy_percent`, `pp_dpm_sclk`) were **not found** at `/sys/class/drm/card1/device/` — this system has multiple DRM card nodes (`card1`, `card2` with many connectors including `eDP-1`); the correct card node for the iGPU device wasn't disambiguated this pass. **Status: UNKNOWN, not unsupported** — recheck via `udevadm info` / PCI bus matching at implementation time.

### NVIDIA dGPU — GeForce RTX 4050 Laptop
- Proprietary driver stack loaded: `nvidia`, `nvidia_drm`, `nvidia_modeset`, `nvidia_uvm`. `nvidia-powerd.service` active.
- `nvidia-smi --query-gpu=name,temperature.gpu,utilization.gpu,clocks.gr,memory.used,memory.total,power.draw` returns clean values for all fields (temp, utilization, clock, VRAM used/total, power draw). No elevated privilege needed.

## RAM

- `/proc/meminfo` / `free -h` — standard, fully supported.
- `hwmon6` name `spd5118` is DIMM SPD **temperature** (45.2°C observed), not a usage metric — do not conflate with RAM usage.

## Battery — BAT1

- `power_supply/BAT1/uevent`: `STATUS`, `PRESENT`, `TECHNOLOGY`, `CYCLE_COUNT`, `VOLTAGE_MIN_DESIGN`, `VOLTAGE_NOW`, `CURRENT_NOW`, `CHARGE_FULL_DESIGN`, `CHARGE_FULL`, `CHARGE_NOW`, `CAPACITY`, `CAPACITY_LEVEL`, `MODEL_NAME=AP21D8M`, `MANUFACTURER=LGC`, `SERIAL_NUMBER`.
- Cross-checked with `upower -i` — percentage, energy, voltage, capacity-level all agree.
- Power draw is derivable as `VOLTAGE_NOW × CURRENT_NOW` (both present); `upower`'s `energy-rate` already does this and reported `0 W` while fully charged and idle, which is expected, not a bug.
- **Charge limit**: `charge_control_end_threshold` and `charge_control_start_threshold` **do not exist** under `/sys/class/power_supply/BAT1/` (confirmed by direct listing — full file set enumerated above; neither file present). **UNSUPPORTED** on this firmware/kernel combination.

## Fans

- Every `hwmon` device on this system was enumerated (`hwmon0`–`hwmon7`: `ACAD`, `acpitz`, `BAT1`, `nvme`, `amdgpu`, `k10temp`, `spd5118`, `mt7921_phy0`). **None expose `fan*_input` or `pwm*`.**
- No `nct6775`, `it87`, or `ec_sys` kernel modules loaded that might otherwise expose EC-driven fan sensors.
- **Fan RPM read: UNSUPPORTED. Fan control: UNSUPPORTED.** No in-tree kernel interface currently exposes fan data on this machine.
- **Relevant to this exact unit**: this machine's BIOS (V1.20, see Identity) is past the ≥1.15 EC-firmware-regression range reported in Acer Community threads (see Third-party prior art below) that Acer's own 1.17 update only partially addressed. Whether V1.20 fixes, retains, or is irrelevant to that regression on this unit is unverified — it's an EC-level fan-sensor/PWM bug, not something the current UNSUPPORTED sysfs state can observe either way. Noted here so a future `predator_v4=1` experiment isn't misread if fan behavior turns out inconsistent even after activation.

## Thermal zones / cooling devices

- One ACPI thermal zone: `thermal_zone0` type `acpitz`, reporting a package-level temp (80°C observed at time of check — higher than `k10temp`'s Tctl, consistent with ACPI's typically-conservative zone reading).
- 16 `cooling_device*` entries, all `type=Processor` (P-state throttling steps), `cur_state=0`/`max_state=3` — standard CPU thermal throttling control devices, not a fan.

## Power profile

- `/sys/firmware/acpi/platform_profile` and `platform_profile_choices`: **absent**. No ACPI-firmware-backed platform profile on this machine.
- `power-profiles-daemon` is active (`systemctl is-active` → `active`) and `powerprofilesctl list` shows `performance`, `balanced`, `power-saver`. `performance`'s only listed driver is `CpuDriver: amd_pstate`; `balanced`/`power-saver` list `PlatformDriver: placeholder` — meaning the profile abstraction is backed only by CPU EPP switching (`amd_pstate`), not a real Acer firmware profile.
- **Conclusion**: OS-level power-profile switching is usable, but **not flatly `SUPPORTED`** — upstream `power-profiles-daemon` documentation confirms that when no real cpufreq/platform_profile backend exists, PPD silently runs a **"placeholder" driver**: the three profiles stay switchable over D-Bus but are no-ops underneath. Our `balanced`/`power-saver` `PlatformDriver: placeholder` finding is exactly this state. Correct classification: `HARDWARE_DEPENDENT` for read (the D-Bus API works, but whether it does anything depends on which driver backs the active profile). True Acer-firmware thermal-profile switching is **UNSUPPORTED/UNKNOWN** via any standard kernel path found.
- **Write privilege — verified empirically, M3**: `cat /usr/share/dbus-1/system.d/org.freedesktop.UPower.PowerProfiles.conf` shows `<policy context="default"><allow send_destination="org.freedesktop.UPower.PowerProfiles" .../></policy>` — any user may call the interface, including the `Properties` interface that sets `ActiveProfile`. Confirmed live: `powerprofilesctl set balanced` (run as the normal, non-root user account, no `sudo`) succeeded immediately with **no polkit prompt**, then restored to `performance`. So write is **`SUPPORTED`, not `REQUIRES_PRIVILEGE`** — this corrects the assumption made before M3 that PPD writes need elevated privilege. It's plain D-Bus system-bus policy (matching `asusctl`'s model, not `system76-power`'s polkit model — see architecture.md, this also resolves that milestone's open decision, which turned out to be moot for M3: PPD is already permissive, so NitroControl adds no privilege layer of its own here).
- **Real D-Bus contract** (confirmed from upstream `power-profiles-daemon` docs, for implementation): bus name/interface `org.freedesktop.UPower.PowerProfiles`, object path `/org/freedesktop/UPower/PowerProfiles`; methods `HoldProfile(profile, reason, application_id) -> cookie` / `ReleaseProfile(cookie)`; properties `ActiveProfile`, `Profiles` (includes per-profile driver info — this is how to detect the placeholder case), `PerformanceDegraded`, `ActiveProfileHolds`. Older distro builds may only register the legacy name `net.hadess.PowerProfiles` at the same path shape — a client should probe the new name first and fall back to the legacy one, never hardcode a single name.

## Acer WMI (`acer_wmi`)

- Module loaded (in-tree driver, author Carlos Corbacho), bound to platform device `acer-wmi`. `modinfo` shows it aliases three WMI GUIDs: `67C3371D-95A3-4C37-BB61-DD47B491DAAB`, `6AF4F258-B401-42FD-BE91-3D4AC2D7C0D3`, `676AA15E-6A47-4D9F-A2CC-1E6D18D14026`.
- Kernel log confirms: `acer_wmi: Acer Laptop ACPI-WMI Extras`, `Function bitmap for Communication Button: 0x1`, and an `Acer WMI hotkeys` input device — this is a **working, verified** hotkey-event interface.
- Module parameters present but inert for this model: `predator_v4=N`, `ec_raw_mode=N`, `cycle_gaming_thermal_profile=Y`, `force_series=0`, `force_caps=-1`, plus legacy `mailled`/`threeg`/`brightness` init-only options. None of these expose a live fan/RGB/thermal-profile control surface for the ANV15-41 **as currently loaded**.
- `/sys/devices/platform/acer-wmi/` contains only generic driver files (`uevent`, `modalias`, `power/*`) — **no custom control attributes** while `predator_v4=N`.
- 18 WMI device GUIDs total exist under `/sys/bus/wmi/devices/` on this system. Four are now **identified with authoritative source**, confirmed directly against mainline `drivers/platform/x86/acer-wmi.c` (kernel source, not guesswork):

  | GUID | Kernel constant | Purpose |
  |---|---|---|
  | `67C3371D-95A3-4C37-BB61-DD47B491DAAB` | `AMW0_GUID1` | Legacy AMW0 interface (older Acer laptops) |
  | `6AF4F258-B401-42FD-BE91-3D4AC2D7C0D3` | `WMID_GUID1` | General WMID get/set device status (wireless, bluetooth, brightness, etc.) |
  | `676AA15E-6A47-4D9F-A2CC-1E6D18D14026` | `ACERWMID_EVENT_GUID` | WMI event GUID — hotkey/event notifications; matches our verified hotkey behavior |
  | `7A4DDFE7-5B5D-40B4-8595-4408E0CC7F56` | `WMID_GUID4` | **Gaming-specific interface** backing in-tree `acer_predator_v4_platform_profile_ops` (thermal/turbo/fan profile switching on Predator/Nitro "v4" laptops) |

  **`WMID_GUID4` is present in this machine's own WMI device enumeration** (`7A4DDFE7-...-11`) — the firmware exposes the gaming interface, but `acer_wmi` hasn't activated it because `predator_v4=N` and this machine's DMI string is not in the kernel's quirk table (upstream-confirmed models for this ops struct are only PH315-53, PH16-72, PT14-51, AN515-58 — no Nitro V15/ANV15-41 entry, per LKML patch history). This is the **single most actionable lead for future fan/thermal-profile work**: reloading `acer_wmi` with `predator_v4=1` (or a matching `force_series` value) is a plausible, fully in-tree path to real profile/fan control — no out-of-tree module, no GPL-forking, no Secure Boot signing concern. **Corroborating evidence**: the Rust project `luizjr/nitro-sense-linux` (Tauri+React, GPL-3.0, targets `AN515-58` — one of the four models upstream-confirmed for `predator_v4`) controls fans by writing standard `hwmon` `pwm1`/`pwm2` files once the ACPI platform-profile interface is active — i.e. on a model where the quirk matches, `predator_v4` doesn't expose a bespoke API, it activates ordinary in-tree `hwmon`/`platform_profile` sysfs nodes. That's the concrete shape to look for post-reload: new `pwm*`/`fan*_input` files under `/sys/class/hwmon`, not a custom interface. It is a live module-reload experiment and is **deferred to a milestone requiring its own explicit user consent** (see `roadmap.md` M5+), not attempted during discovery.

  The remaining 14 of 18 GUIDs stay **unidentified**, unbound to any driver beyond generic `wmi_bmof`/passthrough attributes — documented as present, not acted upon. No capability should be built against an unidentified GUID without kernel-source or datasheet backing.
- `/sys/kernel/debug/acer-wmi` exists but requires root (`debugfs`, standard restriction) — not accessed this pass since read-only-without-privilege was prioritized; may be revisited under `REQUIRES_PRIVILEGE` in a later milestone with explicit user consent.

### `predator_v4=1` experiment — run 2026-09-03, with explicit user consent (M5)

The reload experiment flagged above as the most actionable lead was run for real, on this exact machine, with the user confirming each step live. Full sequence: `sudo rmmod acer_wmi` → `sudo modprobe acer_wmi predator_v4=1` → inspect → `sudo rmmod acer_wmi` → `sudo modprobe acer_wmi` (back to no param). The module reloaded cleanly both directions, no kernel errors, no crash, no lingering state — confirms this stays a fully reversible, in-tree, no-DKMS, no-Secure-Boot-concern action, as predicted.

**Result: real, working, partial fan and thermal-profile control appeared.**

- A new hwmon device, `hwmon8` name `acer` (backed by `/sys/devices/platform/acer-wmi`), appeared immediately: `fan1_input`, `fan2_input`, `temp1_input`, `temp2_input`, `temp3_input` — all **read-only** (no `pwm*` file was present; this activates monitoring, not direct PWM writes). Baseline readings: fan1 2736 RPM, fan2 2579 RPM, temp1 56.0°C, temp2 52.0°C, temp3 51.0°C — plausible, matches `sensors`-class values seen elsewhere in this report, not placeholder/zero data.
- `/sys/firmware/acpi/platform_profile` (previously entirely absent) came into existence, default value `balanced`, with `platform_profile_choices` = `low-power quiet balanced balanced-performance performance`.
- **Writing to `platform_profile` produces a real, measurable fan-speed change** — this is the actual control surface, not `pwm*`:

  | profile written | write result | fan1 RPM | fan2 RPM | temp1 |
  |---|---|---|---|---|
  | `balanced` (baseline, unwritten) | — | 2736 | 2579 | 56.0°C |
  | `quiet` | succeeded | 2542 | 2316 | 55.0°C |
  | `low-power` | succeeded | 2539 | 2328 | 54.0°C |
  | `balanced-performance` | succeeded | **4030** | **3711** | 60.0°C |
  | `performance` | **failed** — `tee: Input/output error` (EIO from the kernel/ACPI layer, not a permissions issue; profile value did not change) | n/a | n/a | n/a |
  | `balanced` (restore) | succeeded | — | — | — |

  `balanced-performance` moving fan1 from 2736→4030 RPM is a large, clearly-causal jump — the strongest positive hardware-control signal this project has found so far, on any capability. 4 of the 5 documented profile values are writable and produce distinct fan behavior; `performance` alone rejects the write at the firmware/ACPI level.
- **Interpretation of the `performance` EIO — root-caused via mainline kernel source** (`drivers/platform/x86/acer-wmi.c`, follow-up research pass): `predator_v4=1` unconditionally selects `quirk_acer_predator_v4` in `find_quirks()` with no DMI check at all, and `acer_predator_v4_platform_profile_probe()` unconditionally advertises all 5 profile bits for any machine using this quirk — there's no per-model capability gating on the Linux side. Sysfs `performance` maps internally to `ACER_PREDATOR_V4_THERMAL_PROFILE_TURBO` (0x05), sent via `WMID_gaming_set_misc_setting()` → `WMI_gaming_execute_u64()` on `WMID_GUID4`. The function returns `-EIO` when the EC/firmware's own response carries a non-zero status byte for that value — **the rejection happens in the EC/firmware, not in Linux**. The kernel does define `ACER_WMID_MISC_SETTING_SUPPORTED_PROFILES` (0x000A) to query a real per-model supported-profile bitmap, but the driver's own source comment calls it `/* Unreliable on some models */` and doesn't use it in the profile-registration path — profiles are exposed unconditionally, accepting some will EIO on hardware that doesn't implement Turbo.
- **AC-power-gating hypothesis tested and ruled out**: a platform-driver-x86 mailing-list thread (SungHwan Jung, re: Predator/Nitro WMI) states Acer's own EC restricts high-performance/Turbo modes to AC power on some models, and a Windows-side Acer Community report for a Nitro V 15 shows NitroSense greying out Performance/Turbo unless on AC with battery >60%. Retested live on this machine at **AC connected, battery 98%, charging** — `performance` write still failed with the identical `tee: Input/output error`. AC/battery state is not the blocker here.
- **Independently corroborated on sibling hardware, not just this unit**: `PXDiv/Div-Acer-Manager-Max` issue #173 reports the **identical** `[Errno 5] Input/output error` writing `performance` on an **Acer Nitro ANV15-51** (Arch, kernel 6.13.10) — same 5-value profile list, same fallback to `balanced-performance`, everything else (fan control via their separate driver, battery limiter, RGB) working. Issue #199 (ANV15-52, Fedora) reports even `get` failing. Issue #119 (older Nitro 5 AN515-58) shows the same EIO pattern on read and write. No maintainer root-cause or fix is recorded in any of these. Also checked: `nitro_v4=1` (the quirk `Div-Linuwu-Sense` maps ANV15-41/-51 to) funnels through the identical `acer_predator_v4_platform_profile_set()` code path as `predator_v4=1` — not a parameter worth trying as an alternative, same EIO expected.
- **Conclusion**: `performance`/Turbo is very likely a firmware/EC characteristic this Nitro V15-class hardware simply doesn't implement (Turbo has historically been Predator-branding, physical-button-gated), not a fixable driver-quirk-selection or power-state problem. **4 of 5 profiles is treated as the practical ceiling for this capability on this hardware**, not a temporary limitation.
- **Not yet tested**: `temp2_input`/`temp3_input` behavior across profiles (only `temp1` was sampled during the write test), whether `acer_wmi`'s hotkey button-profile-cycle behavior changes with `predator_v4=1` active, and whether `power-profiles-daemon` would adopt this new `platform_profile` node automatically after a `power-profiles-daemon` service restart (it did **not** pick it up live without a restart — `powerprofilesctl list` still showed `PlatformDriver: placeholder` for `balanced`/`power-saver` while the experiment was running).
- **Capability status update** (see Capability Matrix below): Fan RPM read and Acer-firmware power profile move from `UNSUPPORTED` (absence confirmed) to `HARDWARE_DEPENDENT` — real and working, but **only when `acer_wmi` is loaded with `predator_v4=1`**, no longer this machine's default boot state as of the persistent config below (2026-09-04).

### Persistent `predator_v4=1` + unprivileged write — applied on this machine 2026-09-04, with explicit user consent

Both items `roadmap.md` had flagged as needing their own separate go-ahead were decided and applied for real, live, on this machine (not just designed) — full copy-paste steps in `docs/optional-setup.md` for other users of this project.

- **`/etc/modprobe.d/nitrocontrol-acer-wmi.conf`**: `options acer_wmi predator_v4=1`. `acer_wmi` isn't in this machine's initramfs, so no `mkinitcpio` regeneration was needed. Verified by reloading the module *without* passing the parameter explicitly (`sudo rmmod acer_wmi && sudo modprobe acer_wmi`) and confirming `/sys/module/acer_wmi/parameters/predator_v4` still read `Y` — proves the config file is doing the work, not leftover shell state.
- **New finding while wiring the udev rule**: this kernel also exposes a proper class device at `/sys/class/platform-profile/platform-profile-0/` (`profile`/`choices`/`name` attributes mirroring the legacy `/sys/firmware/acpi/platform_profile` NitroControl's code actually uses), with `SUBSYSTEM=="platform-profile"` — a real, udev-matchable device, unlike the legacy firmware path. Used purely as the udev trigger; the rule's `RUN+=` action relaxes permission on the legacy path itself rather than requiring any `nitroctl-core` code change. (Worth revisiting as the code's *primary* target in a future pass, once/if the legacy firmware path is ever deprecated upstream — not necessary today, both work.)
- **`/etc/udev/rules.d/90-nitrocontrol-acer-platform-profile.rules`**: `SUBSYSTEM=="platform-profile", KERNEL=="platform-profile-0", RUN+="/usr/bin/chgrp wheel /sys/firmware/acpi/platform_profile", RUN+="/usr/bin/chmod 664 /sys/firmware/acpi/platform_profile"`. Verified: `stat -c '%A %U:%G' /sys/firmware/acpi/platform_profile` → `-rw-rw-r-- root:wheel`, exactly as designed.
- **Functional end-to-end confirmation**: `nitroctl acer-profile set balanced-performance` and `nitroctl acer-profile get`, both **with no `sudo`**, both succeeded — the actual deliverable this project's user wanted (their own words: "balanced-performance ... works really well," now usable as a daily driver, `sudo`-free, surviving reboots by construction — the persistent-config-plus-udev-rule mechanism applies unconditionally at every module load, boot included, though an actual reboot wasn't performed this session to observe it directly).
- These two files live outside the git repo (`/etc/...` on this specific machine) — NitroControl-the-program still never touches them itself, matching `architecture.md`'s M5 design stance. `docs/optional-setup.md` documents them as a manual, copy-paste path for other users, not something `nitroctl` runs on their behalf.

## Third-party prior art (evidence, not endorsement)

### On this machine: DAMX

A community tool, **DAMX v0.5.2** ("Div Acer Manager Max", installed manually at `/opt/damx`, service `damx-daemon.service` running as root), is already attempting Acer fan/RGB/thermal control on this machine. Its own log is corroborating evidence for this report:

```
ERROR - linuwu_sense module not found. Please install the linuwu_sense driver first.
WARNING - Unknown laptop type detected, attempting driver restart...
INFO - Detected laptop type: UNKNOWN
INFO - Four-zone keyboard: No
INFO - ENEK5130 HID RGB device: Not found
INFO - Available features:  (empty)
```

DAMX depends on an **out-of-tree** kernel module (`linuwu_sense`, specifically the `PXDiv/Div-Linuwu-Sense` fork) that is not installed on this system, and even its fallback detection does not recognize ANV15-41. This independently corroborates the fan/RGB findings above. **Interoperability risk**: if a user runs both DAMX and NitroControl, both may attempt to use the same WMI/EC resource. NitroControl must not attempt to disable or interfere with DAMX; this is flagged as an open risk, not solved in v1.

**Important discrepancy**: DAMX's own upstream `Compatibility.md` (github.com/PXDiv/Div-Acer-Manager-Max) lists **ANV15-41 as "Fully/Officially Supported."** This machine's actual runtime state (`Detected laptop type: UNKNOWN`) directly contradicts that claim. DAMX's own FAQ attributes such gaps to DMI-quirk-table mismatches and "tested on Ubuntu only" status. **Process lesson, now with a concrete example**: a third-party compatibility table is never sufficient evidence — only this machine's own verified state counts (spec.md COMPAT-002).

**Update (research pass, 2026-09-03/04)**: DAMX's `Compatibility.md` has since been revised further to claim ANV15-41 is **"Full — Stable with all features working"** — an even stronger claim than before, and still contradicted by this machine's own state above. Independent evidence surfaced this pass makes the caveat concrete rather than hypothetical:

- Multiple Acer Community threads (discussions #724289, #729886, #733886, #736128, #738006, #741896 on community.acer.com) describe a **widespread EC firmware regression starting with BIOS ≥1.15** on ANV15-41/ANV15-51/ANV16-41: the embedded controller loses PWM/fan-sensor reads and fails into a 100%-fan lockup, ignoring all software input — including Windows NitroSense. Acer acknowledges the bug; BIOS 1.17 targeted "power setting parameters" with no confirmed universal fix in these threads. This is a **firmware-level regression, not a Linux driver gap** — it would defeat DAMX/linuwu_sense-style fan control the same way it defeats Windows software, independent of driver quality, and its presence depends on this exact unit's BIOS/EC revision.
- `nbfc-linux` issues [#219](https://github.com/nbfc-linux/nbfc-linux/issues/219) (ANV15-51-7037, i7-13620H/RTX4050) and [#188](https://github.com/nbfc-linux/nbfc-linux/issues/188) (closed "not planned"): fan RPM *reads* work via generic EC probing, but *writes* are silently ignored on this EC generation — consistent with the write path being gated behind proper WMI method calls rather than raw EC port pokes, which is the whole reason WMI-based drivers like `linuwu_sense` exist. PXDiv's own suggested workaround in #188 is "use DAMX instead" — not a claim that DAMX is independently verified to work, undercutting the "Full — Stable" claim further.

Net: a compatibility-table claim getting *stronger* over time while independent, contemporaneous user reports describe the opposite is exactly the failure mode COMPAT-002 exists to guard against. Re-run this section's checks (`## Fans`) if this project's own BIOS is ever updated, per the re-verification triggers below.

**Update (2026-09-04, `predator_v4`-adjacent research)**: two data points cut the other way and are recorded for balance, not to walk back the skepticism above — a compatibility table is still not evidence on its own:
- `frederik-h/acer-wmi-battery` issue #92: a user reports the battery charge-limiter WMI method **does** work on an ANV15-41, Ubuntu 24.04.2 — no technical detail given, unverified on this machine, but a real positive report on the exact model for a different capability (battery, not fan).
- `Div-Acer-Manager-Max` issue #168: a Nitro V15 user running the working `linuwu_sense`-based fan control reports it as real but **limited — a ~2000 RPM floor that can't be reduced further**, even when it "works." A useful caveat on what "Full — Stable" is actually worth in practice, even taken at face value.
- No CachyOS-specific `acer_wmi` kernel patches were found (checked their kernel packaging) — this machine runs the same upstream driver any Arch-based distro would ship; nothing CachyOS-specific to account for in any of this section's results.

### `frederik-h/acer-wmi-battery` — live discovery test, run 2026-09-04, with explicit user consent

Per the research above, issue #92 ("works on ANV15-41") was the strongest lead but unconfirmed by any maintainer and unverified independently. Rather than adopt the driver into NitroControl on that evidence alone, it was built and tested locally as **pure discovery — not integrated into `nitroctl-core`, not installed persistently, no NitroControl code changed.** This is the project's first time loading any out-of-tree module.

- **Found and patched a real bug before testing**: `get_battery_health_control_status()` and `set_battery_health_control()` both dereferenced the WMI response buffer (`obj->buffer.pointer`) *before* checking `obj->buffer.length` matched the expected struct size — an out-of-bounds heap read in kernel space if firmware ever returns a short buffer (`get_battery_information()`, defined earlier in the same file, does the check-then-read correctly, for comparison). Reordered both to check length first. Not yet reported upstream — worth doing if this driver is ever revisited.
- **Build note**: this machine's CachyOS kernel (`7.2.2-1-cachyos`) is Clang-built; a plain `make` fails (`gcc: error: unrecognized command-line option`) — needed `make LLVM=1`. Resulting module's `vermagic` matched the running kernel exactly; Secure Boot is disabled on this machine (`bootctl status`), so no MOK signing was needed.
- **Result: real, working, confirmed on this exact unit** — independently corroborates and extends issue #92, not just repeats it:
  - `insmod` succeeded; dmesg: `acer_wmi_battery: available modes: health mode, calibration mode` — **both** WMI-exposed features probe successfully on ANV15-41, not just the one issue #92 mentioned.
  - Reads: `health_mode` = `0`, `calibration_mode` = `0` (both available, both off by default), `temperature` = `29500` → 29.5°C, a plausible real battery temperature (matches the driver's Smart-Battery-Data-spec unit conversion).
  - Write tested (battery was at 100%/Full, a safe time — no forced charge-stop triggered by the test): `echo 1 > health_mode` → dmesg `acer_wmi_battery: enabled health mode`, readback confirmed `1`; `echo 0 > health_mode` → disabled again, module `rmmod`'d cleanly, sysfs path gone, `lsmod` confirms unloaded. Full write→verify→revert loop worked exactly as the driver claims.
- **What this does and doesn't establish**: this confirms the *hardware* genuinely implements Acer's battery-health WMI interface, and that (a locally-patched build of) this specific driver talks to it correctly on ANV15-41. It does **not** by itself justify adopting this out-of-tree module into NitroControl — that's a separate decision (DKMS packaging, ongoing patch maintenance, whether to upstream the buffer-check fix, the project's standing "stay independent of out-of-tree modules for v1" position in `roadmap.md`) not made by this discovery pass.

### M6/FR-008 implementation — live verification through the real `nitroctl` binary, run 2026-09-04, with explicit user consent

Follow-up to the discovery pass above: adoption was decided (`roadmap.md` M6), a maintained fork built ([`bwz-kk/acer-wmi-battery`](https://github.com/bwz-kk/acer-wmi-battery), carrying the heap-read fix), `nitroctl-core`/`cli`/`gui` implemented (TDD, PR #8), and this section closes the loop with a real end-to-end run — same standard FR-007's M5 verification set (not just `tee`, the actual binary).

- Battery at 100%/Full — same safe condition as the discovery pass.
- Fresh build of the fork (`make LLVM=1`, this machine's kernel `7.2.2-1-cachyos`) loaded via `insmod`. `dmesg`: `acer_wmi_battery: available modes: health mode, calibration mode`. Default state confirmed via the real binary: `nitroctl battery-limit get` → `Battery charge limit: off` (exit 0, no `sudo`) — reads need no privilege, matching FR-007's read/write asymmetry.
- Permission bits: `health_mode` is root-owned (`rw-r--r--`, no group/other write) — same shape as `/sys/firmware/acpi/platform_profile` before FR-007's udev rule, confirmed live rather than assumed from that precedent.
- Unprivileged write correctly denied: `nitroctl battery-limit set on` (no `sudo`) → `Battery charge limit write was denied -- this needs root, or a udev rule relaxing permission on /sys/bus/wmi/drivers/acer-wmi-battery/health_mode (see docs/optional-setup.md)`, exit 3.
- Privileged write, through the real binary, not `tee`: `sudo nitroctl battery-limit set on` → `Battery charge limit turned on`, exit 0; `dmesg` confirms `acer_wmi_battery: enabled health mode`; unprivileged `nitroctl battery-limit get` immediately after → `Battery charge limit: on`, exit 0 — matches raw sysfs exactly.
- Restored to default and unloaded: `sudo nitroctl battery-limit set off` → `Battery charge limit turned off`, exit 0; `dmesg` confirms `acer_wmi_battery: disabled health mode`; `sudo rmmod acer-wmi-battery` → sysfs path gone (`No such file or directory`), confirming clean unload.
### M6/FR-008 persistent setup — DKMS + udev rule, run 2026-09-04, with explicit user consent

Follow-up to the live-verification pass above: the fork built via DKMS (`dkms.conf` in the fork repo, `dkms add`/`build`/`install`), MOK-signed automatically as part of DKMS's standard flow (Secure Boot disabled on this machine, so no manual enrollment needed), `dkms status` confirms `installed`. `/etc/modules-load.d/nitrocontrol-acer-wmi-battery.conf` makes it autoload at boot.

- **The FR-007/M5 udev pattern didn't transfer directly** — worth recording as a real, model-independent finding about how WMI-bus method-type devices behave, not just a Nitro V15 quirk:
  - The WMI device node itself (`/sys/bus/wmi/devices/79772EC5-04B1-4BFD-843C-61E7F77B6CC9-7`, `DEVTYPE=method`) carries no `DRIVER=` uevent property and emits **no uevent at all** on driver bind/unbind — confirmed live via `udevadm monitor` across a real `rmmod`/`modprobe` cycle (zero events for that device). The first rule attempt (`SUBSYSTEM=="wmi", ACTION=="bind", DRIVER=="acer-wmi-battery"`) installed without error but silently never fired.
  - The **module** kobject's `add` event (`SUBSYSTEM=="module", KERNEL=="acer_wmi_battery"`) does fire reliably, but too *early*: the kernel sends it from `mod_sysfs_setup()` before `do_init_module()` runs the driver's `module_init`/probe, so `health_mode` doesn't exist yet when the rule's `RUN+=` executes — confirmed live (permissions stayed `root:root` after a real reload with this rule installed).
  - The **driver** kobject's `add` event (`SUBSYSTEM=="drivers", KERNEL=="acer-wmi-battery"`) is the one that works: `bus_add_driver()` (`drivers/base/bus.c`) only emits it after `driver_attach()` has already run probe, so `health_mode` is guaranteed to exist. Confirmed live: `stat` shows `-rw-rw-r-- root:wheel` after a real `rmmod`/`modprobe` cycle, and unprivileged `nitroctl battery-limit set on` succeeds (exit 0, no `sudo`).
- Final rule installed at `/etc/udev/rules.d/90-nitrocontrol-acer-wmi-battery.rules`: `SUBSYSTEM=="drivers", KERNEL=="acer-wmi-battery", ACTION=="add", RUN+=".../chgrp wheel .../health_mode", RUN+=".../chmod 664 .../health_mode"`.
- Documented as copy-paste-only in `docs/optional-setup.md`'s new "battery charge limit (M6, FR-008)" section — NitroControl itself never installs any of this automatically, same SAFE-001/SAFE-002 stance as FR-007's M5 setup.

### M7/FR-009 implementation — toggle-only live verification through the real `nitroctl` binary, run 2026-09-04, with explicit user consent

Follow-up to M6: `nitroctl-core`/`cli`/`gui` implemented `calibration_mode` (TDD, its own branch+PR), and this section records the real hardware run — deliberately narrower in scope than FR-007/FR-008's standard, per `architecture.md`'s M7 design section: this verifies the sysfs attribute responds correctly to a write/readback/immediate-disable round trip, **not** that a full multi-hour discharge/recharge cycle completes correctly on this hardware. No rebuild needed — same already-loaded `bwz-kk/acer-wmi-battery` module as M6, `calibration_mode` was already present alongside `health_mode`.

- Starting state: `calibration_mode` root-owned (`rw-r--r-- root:root` — the existing udev rule only relaxes `health_mode`'s permission, not this attribute's, so `set` needs `sudo` as expected), read confirms `off` with no privilege needed.
- `sudo nitroctl battery-calibrate set on` → `Battery calibration mode turned on -- this starts a multi-hour discharge/recharge cycle...` (the CLI's explicit caution, `commands.rs`), exit 0. `dmesg` confirms two real state changes, not one: `acer_wmi_battery: enabled calibration mode` **and** `acer_wmi_battery: disabled health mode` — the EC disables `health_mode` as a side effect of the WMI call, exactly as the driver's docs describe, confirmed live rather than assumed. Unprivileged `nitroctl battery-calibrate get` immediately after → `on`.
- **New finding, not documented anywhere in the community write-ups consulted during SPECIFY**: `sudo nitroctl battery-calibrate set off` → `dmesg` shows `acer_wmi_battery: disabled calibration mode` **and** `acer_wmi_battery: enabled health mode` — turning calibration off didn't just stop it, the EC automatically **re-enabled `health_mode`** on its own. The driver's C source (`calibration_mode_store` in `acer-wmi-battery.c`) has no code that touches `health_mode` directly — it only issues the WMI calibration call and re-reads status (`update_state()`); the health-mode toggle is a real firmware-level side effect of the EC, not driver logic. Confirmed live, both directions, on this one ANV15-41 test unit only — not claimed to generalize to other models or even other units of this model (the LWN submitter's own Aspire A315-510P didn't behave consistently enough for them to keep `calibration_mode` at all). This is an EC/firmware-level observation, not a NitroControl software behavior: `nitroctl`'s own code never touches `health_mode` when writing `calibration_mode` (see `battery_calibration.rs`'s test coverage) — it reads sysfs fresh each call, so it reports whatever the EC actually did without needing to model it. Unprivileged `nitroctl battery-calibrate get` immediately after → `off`.
- Elapsed time between `set on` and `set off`: well under a minute — no real discharge/recharge cycle was allowed to start or run, per the M7 design decision. **This does not establish that a full cycle works correctly on this hardware** — that remains the same open gap the in-tree submission's author hit on their unit; if a user runs a full cycle and reports back, that's new evidence for a future pass.

### Wider ecosystem, surveyed for completeness (none installed here, none adopted)

| Project | License | Language | Kernel dependency | ANV15-41 support status |
|---|---|---|---|---|
| `0x7375646F/Linuwu-Sense` (upstream of the fork DAMX uses) | GPL-3.0 | C | Out-of-tree, patches `acer_wmi`; no DKMS | Not listed (PHN16-71 is the flagship "fully supported" model) |
| `PXDiv/Div-Linuwu-Sense` (DAMX's fork) | GPL-3.0 | C | Out-of-tree; DMI-quirk-table detection (`acer_quirks[]`); force-load via `nitro_v4=1`/`predator_v4=1`/`enable_all=1` module params as an unsupported-model workaround | Not in quirk table |
| `frederik-h/acer-wmi-battery` | GPL-2.0 | C | Out-of-tree WMI driver (`/sys/bus/wmi/drivers/acer-wmi-battery/{health_mode,calibration_mode,temperature}`); AUR has `-dkms`/`-dkms-git` packages | Not in `MODELS.md` (sibling ANV15-51 is listed). **Update (2026-09)**: a cleaned-up port onto standard kernel battery-hook APIs is now a real, dated upstream candidate — Jelle van der Waa's v2 series posted to platform-driver-x86@vger.kernel.org, Cc Hans de Goede/Ilpo Järvinen ([LWN, 2026-01-25](https://lwn.net/Articles/1055804/)); still in mailing-list review, not merged as of this pass. Tested by the submitter only on an Aspire A315-510P; `calibration_mode` was deliberately dropped from the submission because it "did not work as expected" even on that unit — further evidence of per-model inconsistency, not a reason to expect it works here untested. **Adopted 2026-09-04 (M6, FR-008)**: NitroControl uses a maintained fork, [`bwz-kk/acer-wmi-battery`](https://github.com/bwz-kk/acer-wmi-battery) (from `9f90d75`), carrying the out-of-bounds heap-read fix found during the M5+ discovery pass (below) plus a `dkms.conf`. The one explicit exception to this project's out-of-tree-independence stance (`roadmap.md` "Out of scope indefinitely", `architecture.md`'s M6 design section) — will switch to the in-tree submission above if/when it merges. |
| `maxco2/acer-battery-wmi` | — | C | Second, independent out-of-tree WMI battery driver (separate codebase from `frederik-h`'s) | New find (2026-09). Listed by the `Battery-Health-Charging` compatibility page alongside `frederik-h/acer-wmi-battery` as covering a different Acer model group — worth trying if `frederik-h`'s driver fails to probe this machine's WMI GUID, since Acer varies the battery WMI interface across model families. Not tested against ANV15-41. |
| `cleyton1986/predator-sense` | GPL-3.0 (product images excluded) | **Rust + GTK4/libadwaita**, own `facer.ko` via DKMS | Ships own kernel module but designed to defer to `linuwu_sense` if already loaded | Not confirmed for this model; closest architectural analog to our own stack |
| `PXDiv/Div-Acer-Manager-Fan-Controls` | — | C module + daemon + Avalonia GUI | Out-of-tree WMI kernel driver; explicitly a fan-control-only rewrite/continuation of the earlier AcerLinuxManager, narrower scope than DAMX | New find (2026-09). No Secure Boot signing story (same unsigned-DKMS problem as `linuwu_sense`/DAMX); docs claim support for "most recent Nitro/Predator models" generically, no ANV15-41-specific confirmation |
| `Order52/linuwu-sense-cli` | GPL-3.0 | C | Fork/CLI-only variant of `linuwu_sense` | New find (2026-09). No ANV15-41-specific evidence found |
| `JafarAkhondali/acer-predator-turbo-and-rgb-keyboard-linux-module` | GPL-3.0 | C module + Python CLI/GUI | Own `facer.c`, no DKMS | Not listed; unmaintained (maintainer no longer has Acer hardware) |
| `hirschmann/nbfc` + `nbfc-linux` | GPL-3.0 | C#/.NET | None — talks to EC directly via `/dev/port`/ACPI EC; per-model hand-written XML config, no auto-detection | No config found for ANV15-41. **Update (2026-09)**: issues [#188](https://github.com/nbfc-linux/nbfc-linux/issues/188)/[#219](https://github.com/nbfc-linux/nbfc-linux/issues/219) confirm this raw-EC-poke approach reads fan RPM but cannot write it on the ANV15-51-generation EC — see DAMX note above |
| `luizjr/nitro-sense-linux` | GPL-3.0 (one file GPL-2.0) | **Rust** (Tauri 2 backend + React frontend) | In-tree `acer_wmi` `platform_profile` + `hwmon` `pwm1`/`pwm2` directly, no custom module; privileged helper via polkit | Targets `AN515-58` exclusively (early-stage, 7 commits); confirms `predator_v4` activation surfaces as ordinary `hwmon`/`platform_profile` nodes, not a bespoke API — see `predator_v4` note above |
| `VictorhMalheiro/electron-nitro-sense` | MIT | Electron | `acpi_ec` direct EC access, a separate `acer-predator-module` for RGB, `amdctl` for CPU undervolt | Targets `AN515-46` only; dormant (4 commits, 0 stars-worth of activity) |

**Licensing note**: every hardware-control kernel module surveyed is GPL-2.0/3.0 — unavoidable for kernel code. Userspace tooling can be licensed independently since sysfs is a non-linking boundary; moot for NitroControl v1 since it depends on none of these.

**Upstream stance note**: an active platform-driver-x86 mailing-list thread ("acer-wmi: Improving WMI support for Predator/Nitro laptops," re: Nitro AN17-41) shows the maintainer (Armin Wolf) pushing back on exposing ad hoc hardware-limitation state through the generic `platform_profile` ABI. If NitroControl ever pursues an upstream contribution (e.g. an ANV15-41 DMI quirk entry) rather than staying purely a consumer, its feature semantics should follow existing `platform_profile` ABI conventions from the start.

**Decision**: NitroControl stays independent for v1 — no fork of or dependency on `linuwu_sense`, DAMX, or `predator-sense`. This keeps the project GPL-module-free, DKMS-free, and Secure-Boot-clean for everything except the one explicit M6 exception below. **Amended 2026-09-04 (M6)**: `acer-wmi-battery` is no longer in this "stays independent" list — a maintained fork ([`bwz-kk/acer-wmi-battery`](https://github.com/bwz-kk/acer-wmi-battery)) was adopted for FR-008 (battery charge limit), a separate, explicit, user-reviewed decision (`roadmap.md` M6, `architecture.md`'s M6 design section) — not a reversal of the stance for the other three projects listed here.

## Keyboard backlight

- No `kbd_backlight`-named LED class device exists anywhere under `/sys` (exhaustive `find` across `/sys` for the pattern turned up nothing).
- Only backlight device present is `amdgpu_bl2` under `/sys/class/backlight/` — this is the **screen** backlight, unrelated to keyboard.
- **UNSUPPORTED** — likely `HARDWARE_DEPENDENT`: this SKU may have a fixed-color, non-OS-visible keyboard backlight toggled purely at the EC/firmware level via a hotkey, with no software control surface.

## Powercap

- `/sys/class/powercap/intel-rapl*` is present. The `intel-rapl` name is the generic Linux driver name for the RAPL-style powercap zone and is a naming artifact — it does not imply an Intel CPU. Not verified further this pass; noted as a possible secondary power-draw data source for a future milestone.

## Capability Matrix

| Capability | Detection | Read | Write | Interface | Verified |
|---|---|---|---|---|---|
| CPU temperature | SUPPORTED | SUPPORTED | N/A | `hwmon` `k10temp` (`Tctl`) | Yes |
| GPU temperature (dGPU) | SUPPORTED | SUPPORTED | N/A | NVML (`nvidia-smi`) | Yes |
| GPU temperature (iGPU) | SUPPORTED | SUPPORTED | N/A | `hwmon` `amdgpu` `temp1` | Yes |
| CPU utilization | SUPPORTED | SUPPORTED | N/A | `/proc/stat` | Yes |
| GPU utilization (dGPU) | SUPPORTED | SUPPORTED | N/A | NVML | Yes |
| GPU utilization (iGPU) | UNKNOWN | UNKNOWN | N/A | expected `drm`/`amdgpu`, card path unconfirmed | No |
| CPU frequency | SUPPORTED | SUPPORTED | N/A | `cpufreq` sysfs | Yes |
| GPU frequency (dGPU) | SUPPORTED | SUPPORTED | N/A | NVML | Yes |
| GPU frequency (iGPU) | UNKNOWN | UNKNOWN | N/A | `pp_dpm_sclk` path unconfirmed | No |
| RAM usage | SUPPORTED | SUPPORTED | N/A | `/proc/meminfo` | Yes |
| VRAM usage (dGPU) | SUPPORTED | SUPPORTED | N/A | NVML | Yes |
| Battery status/charge % | SUPPORTED | SUPPORTED | N/A | `power_supply` BAT1 | Yes |
| Battery charge limit | HARDWARE_DEPENDENT | HARDWARE_DEPENDENT | HARDWARE_DEPENDENT (needs root, or a udev rule the user installs — not shipped by NitroControl) | out-of-tree, adopted for M6: [`bwz-kk/acer-wmi-battery`](https://github.com/bwz-kk/acer-wmi-battery) (fork of `frederik-h/acer-wmi-battery` with the heap-read fix above), `/sys/bus/wmi/drivers/acer-wmi-battery/health_mode`, only present when that module is loaded (NitroControl never loads it on its own — `UNSUPPORTED` under default boot config, same pattern as Fan RPM/Acer power profile) | Yes — verified end-to-end through the real `nitroctl battery-limit get/set` binary (see the M6/FR-008 live-verification section above): default off/no-privilege-needed read, unprivileged write correctly denied, privileged write confirmed via `dmesg` and readback, restore+unload confirmed |
| Battery calibration mode | HARDWARE_DEPENDENT | HARDWARE_DEPENDENT | HARDWARE_DEPENDENT (needs root — no udev rule relaxes this attribute yet, only `health_mode`'s is set up) | same driver/module as Battery charge limit (M7, FR-009), `/sys/bus/wmi/drivers/acer-wmi-battery/calibration_mode` | Toggle mechanics only — see the M7/FR-009 live-verification section above: `set on`/`set off` and the driver's real state transitions (including the undocumented health-mode-re-enable-on-disable side effect) confirmed live through the real binary. **Not verified**: whether a full multi-hour discharge/recharge cycle completes correctly on this hardware — deliberately out of scope for M7's evidence standard (`architecture.md`'s M7 design section); the in-tree submission's author dropped this exact attribute after it "did not work as expected" on their own unit |
| Fan RPM | HARDWARE_DEPENDENT | HARDWARE_DEPENDENT | N/A | `hwmon` `acer` (`fan1_input`/`fan2_input`), only present when `acer_wmi` is loaded with `predator_v4=1` (not this machine's default) | Yes — real RPM values read live during the M5 experiment (see `predator_v4=1 experiment` above); confirmed end-to-end through `nitroctl-core`/`nitroctl fans` (no code change needed — `GenericLinux::fan_rpm()`'s generic hwmon scan already picks it up), output matched raw sysfs exactly (`3032`/`2675` RPM); `UNSUPPORTED` under default boot config |
| Fan control | HARDWARE_DEPENDENT | N/A | HARDWARE_DEPENDENT | `platform_profile` write (`low-power`/`quiet`/`balanced`/`balanced-performance` confirmed working, `performance` fails EIO), only when `predator_v4=1` loaded | Yes — measured real fan-speed change from a `platform_profile` write (2736→4030 RPM); `UNSUPPORTED` under default boot config; no direct `pwm*` write path found |
| Power profile (OS-level) | SUPPORTED | HARDWARE_DEPENDENT | SUPPORTED | `power-profiles-daemon` D-Bus (`org.freedesktop.UPower.PowerProfiles`, fallback `net.hadess.PowerProfiles`) | Yes — `list`/`get`/`set` all exercised live (M3); no privilege needed (D-Bus policy is `context="default"`); `balanced`/`power-saver` run PPD's placeholder backend (switchable, no-op) |
| Power profile (Acer firmware) | HARDWARE_DEPENDENT | HARDWARE_DEPENDENT | HARDWARE_DEPENDENT | `/sys/firmware/acpi/platform_profile`, only present when `acer_wmi` is loaded with `predator_v4=1` (not this machine's default) | Yes — `list`/`get`/`set` all exercised live during the M5 experiment; 4/5 profile values write successfully and change real fan RPM, `performance` fails EIO; node is entirely absent (`UNSUPPORTED`) under default boot config |
| Keyboard backlight | UNSUPPORTED | UNSUPPORTED | UNSUPPORTED | no LED device found | Yes (absence confirmed) |
| Acer hotkeys | SUPPORTED | SUPPORTED (input events) | N/A | `acer_wmi` input device | Yes (kernel log) |
| Acer fan/thermal/RGB profiles | HARDWARE_DEPENDENT (fan/thermal); UNKNOWN (RGB) | HARDWARE_DEPENDENT (fan/thermal) | HARDWARE_DEPENDENT (fan/thermal, partial — 4/5 values) | in-tree `acer_wmi` `WMID_GUID4` via `predator_v4=1`; RGB not tested this pass | Yes — `predator_v4=1` experiment run 2026-09-03, real results recorded above; RGB path still untested |

## Re-verification triggers

Re-run this discovery process (do not assume prior results still hold) if:
- BIOS is updated (current: V1.20).
- Kernel is upgraded significantly (current: `7.2.2-1-cachyos`).
- `linuwu_sense`, `acer-wmi-battery`, or any out-of-tree Acer driver is installed on this machine.
- `acer_wmi` is upstream-patched with new GUID support, or `acer-wmi-battery`'s in-tree submission lands.
- The `predator_v4=1` module-reload experiment (roadmap.md M5+) is ever run — record its outcome here regardless of result.
- Any third-party project's compatibility table changes or is cited as evidence — re-verify on this machine before trusting it (see DAMX discrepancy above).
