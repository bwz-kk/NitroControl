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

## Thermal zones / cooling devices

- One ACPI thermal zone: `thermal_zone0` type `acpitz`, reporting a package-level temp (80°C observed at time of check — higher than `k10temp`'s Tctl, consistent with ACPI's typically-conservative zone reading).
- 16 `cooling_device*` entries, all `type=Processor` (P-state throttling steps), `cur_state=0`/`max_state=3` — standard CPU thermal throttling control devices, not a fan.

## Power profile

- `/sys/firmware/acpi/platform_profile` and `platform_profile_choices`: **absent**. No ACPI-firmware-backed platform profile on this machine.
- `power-profiles-daemon` is active (`systemctl is-active` → `active`) and `powerprofilesctl list` shows `performance`, `balanced`, `power-saver`. `performance`'s only listed driver is `CpuDriver: amd_pstate`; `balanced`/`power-saver` list `PlatformDriver: placeholder` — meaning the profile abstraction is backed only by CPU EPP switching (`amd_pstate`), not a real Acer firmware profile.
- **Conclusion**: OS-level power-profile switching is usable, but **not flatly `SUPPORTED`** — upstream `power-profiles-daemon` documentation confirms that when no real cpufreq/platform_profile backend exists, PPD silently runs a **"placeholder" driver**: the three profiles stay switchable over D-Bus but are no-ops underneath. Our `balanced`/`power-saver` `PlatformDriver: placeholder` finding is exactly this state. Correct classification: `HARDWARE_DEPENDENT` for read (the D-Bus API works, but whether it does anything depends on which driver backs the active profile), `REQUIRES_PRIVILEGE` for write. True Acer-firmware thermal-profile switching is **UNSUPPORTED/UNKNOWN** via any standard kernel path found.
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

  **`WMID_GUID4` is present in this machine's own WMI device enumeration** (`7A4DDFE7-...-11`) — the firmware exposes the gaming interface, but `acer_wmi` hasn't activated it because `predator_v4=N` and this machine's DMI string is not in the kernel's quirk table (upstream-confirmed models for this ops struct are only PH315-53, PH16-72, PT14-51, AN515-58 — no Nitro V15/ANV15-41 entry, per LKML patch history). This is the **single most actionable lead for future fan/thermal-profile work**: reloading `acer_wmi` with `predator_v4=1` (or a matching `force_series` value) is a plausible, fully in-tree path to real profile/fan control — no out-of-tree module, no GPL-forking, no Secure Boot signing concern. It is a live module-reload experiment and is **deferred to a milestone requiring its own explicit user consent** (see `roadmap.md` M5+), not attempted during discovery.

  The remaining 14 of 18 GUIDs stay **unidentified**, unbound to any driver beyond generic `wmi_bmof`/passthrough attributes — documented as present, not acted upon. No capability should be built against an unidentified GUID without kernel-source or datasheet backing.
- `/sys/kernel/debug/acer-wmi` exists but requires root (`debugfs`, standard restriction) — not accessed this pass since read-only-without-privilege was prioritized; may be revisited under `REQUIRES_PRIVILEGE` in a later milestone with explicit user consent.

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

### Wider ecosystem, surveyed for completeness (none installed here, none adopted)

| Project | License | Language | Kernel dependency | ANV15-41 support status |
|---|---|---|---|---|
| `0x7375646F/Linuwu-Sense` (upstream of the fork DAMX uses) | GPL-3.0 | C | Out-of-tree, patches `acer_wmi`; no DKMS | Not listed (PHN16-71 is the flagship "fully supported" model) |
| `PXDiv/Div-Linuwu-Sense` (DAMX's fork) | GPL-3.0 | C | Out-of-tree; DMI-quirk-table detection (`acer_quirks[]`); force-load via `nitro_v4=1`/`predator_v4=1`/`enable_all=1` module params as an unsupported-model workaround | Not in quirk table |
| `frederik-h/acer-wmi-battery` | GPL-2.0 | C | Out-of-tree WMI driver (`/sys/bus/wmi/drivers/acer-wmi-battery/{health_mode,calibration_mode,temperature}`); AUR has a `-dkms-git` package | Not in `MODELS.md` (sibling ANV15-51 is listed); a cleaned-up version is **under review on the platform-driver-x86 mailing list for mainline inclusion** — the strongest in-tree-future signal found for battery limiting specifically |
| `cleyton1986/predator-sense` | GPL-3.0 (product images excluded) | **Rust + GTK4/libadwaita**, own `facer.ko` via DKMS | Ships own kernel module but designed to defer to `linuwu_sense` if already loaded | Not confirmed for this model; closest architectural analog to our own stack |
| `JafarAkhondali/acer-predator-turbo-and-rgb-keyboard-linux-module` | GPL-3.0 | C module + Python CLI/GUI | Own `facer.c`, no DKMS | Not listed; unmaintained (maintainer no longer has Acer hardware) |
| `hirschmann/nbfc` + `nbfc-linux` | GPL-3.0 | C#/.NET | None — talks to EC directly via `/dev/port`/ACPI EC; per-model hand-written XML config, no auto-detection | No config found for ANV15-41 |

**Licensing note**: every hardware-control kernel module surveyed is GPL-2.0/3.0 — unavoidable for kernel code. Userspace tooling can be licensed independently since sysfs is a non-linking boundary; moot for NitroControl v1 since it depends on none of these.

**Upstream stance note**: an active platform-driver-x86 mailing-list thread ("acer-wmi: Improving WMI support for Predator/Nitro laptops," re: Nitro AN17-41) shows the maintainer (Armin Wolf) pushing back on exposing ad hoc hardware-limitation state through the generic `platform_profile` ABI. If NitroControl ever pursues an upstream contribution (e.g. an ANV15-41 DMI quirk entry) rather than staying purely a consumer, its feature semantics should follow existing `platform_profile` ABI conventions from the start.

**Decision**: NitroControl stays independent for v1 — no fork of or dependency on `linuwu_sense`, DAMX, `acer-wmi-battery`, or `predator-sense`. This keeps the project GPL-module-free, DKMS-free, and Secure-Boot-clean. Revisit only per the concrete triggers in `roadmap.md` M5+.

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
| Battery charge limit | UNSUPPORTED | UNSUPPORTED | UNSUPPORTED | none found | Yes (absence confirmed) |
| Fan RPM | UNSUPPORTED | UNSUPPORTED | N/A | none active; community `acer_nitro_ec` hwmon exists in the OOT module family (not installed) | Yes (absence confirmed) |
| Fan control | UNSUPPORTED | N/A | UNSUPPORTED | none active; in-tree `acer_wmi` `WMID_GUID4`/`predator_v4` path present-but-disabled (untested experiment, see roadmap.md M5+) | Yes (absence confirmed) |
| Power profile (OS-level) | SUPPORTED | HARDWARE_DEPENDENT | REQUIRES_PRIVILEGE | `power-profiles-daemon` D-Bus (`org.freedesktop.UPower.PowerProfiles`, fallback `net.hadess.PowerProfiles`) | Partial — `list` confirmed; `balanced`/`power-saver` run PPD's placeholder backend (switchable, no-op); `set` not yet exercised |
| Power profile (Acer firmware) | UNSUPPORTED | UNSUPPORTED | UNSUPPORTED | `platform_profile` absent | Yes (absence confirmed) |
| Keyboard backlight | UNSUPPORTED | UNSUPPORTED | UNSUPPORTED | no LED device found | Yes (absence confirmed) |
| Acer hotkeys | SUPPORTED | SUPPORTED (input events) | N/A | `acer_wmi` input device | Yes (kernel log) |
| Acer fan/thermal/RGB profiles | UNKNOWN/UNSUPPORTED | — | — | in-tree `acer_wmi` `WMID_GUID4` present but inactive (`predator_v4=N`, no DMI quirk match) | Yes (inactive state confirmed); untested `predator_v4=1` experiment identified for M5+ |

## Re-verification triggers

Re-run this discovery process (do not assume prior results still hold) if:
- BIOS is updated (current: V1.20).
- Kernel is upgraded significantly (current: `7.2.2-1-cachyos`).
- `linuwu_sense`, `acer-wmi-battery`, or any out-of-tree Acer driver is installed on this machine.
- `acer_wmi` is upstream-patched with new GUID support, or `acer-wmi-battery`'s in-tree submission lands.
- The `predator_v4=1` module-reload experiment (roadmap.md M5+) is ever run — record its outcome here regardless of result.
- Any third-party project's compatibility table changes or is cited as evidence — re-verify on this machine before trusting it (see DAMX discrepancy above).
