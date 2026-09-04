# Optional setup: unprivileged Acer-firmware power-profile control

This is a manual, copy-paste setup guide — **NitroControl never runs any of
this itself.** Per `docs/architecture.md`'s M5 design section, the project
deliberately stays detect-only for anything touching boot-time module
parameters or kernel-file permissions (SAFE-001/SAFE-002); this doc exists so
users who want `nitroctl acer-profile set` to work without `sudo`, every
boot, don't have to re-derive the two steps from scratch.

Applies to: Acer Nitro/Predator laptops where `acer_wmi`'s `WMID_GUID4`
gaming interface is present but inactive by default (confirmed on the Nitro
V15 ANV15-41 — see `hardware.md`'s M5 `predator_v4=1` experiment; may or may
not apply to other models, verify on your own machine first per COMPAT-002).

**Do this only after you've confirmed `predator_v4=1` actually does
something useful on your exact model** (`hardware.md`'s experiment steps) —
don't apply this blind.

## 1. Make `predator_v4=1` persist across reboots

`acer_wmi` is a normal loadable module (not baked into the initramfs on a
typical CachyOS/Arch install — verify with `lsinitcpio` on your own image if
unsure), so a `modprobe.d` file is enough; no `mkinitcpio` regeneration
needed.

```bash
echo 'options acer_wmi predator_v4=1' | sudo tee /etc/modprobe.d/nitrocontrol-acer-wmi.conf
```

Verify it actually took effect (reload *without* passing the parameter
explicitly — if this still comes back `Y`, the file is doing the work, not
your shell history):

```bash
sudo rmmod acer_wmi
sudo modprobe acer_wmi
cat /sys/module/acer_wmi/parameters/predator_v4
```

## 2. Relax write permission on `platform_profile`, via udev

`/sys/firmware/acpi/platform_profile` (the file `AcerPlatformProfileBackend`
actually reads/writes) is root-owned by default and isn't itself a proper
udev-matchable device. But modern kernels (verified present here) also
expose a real class device that mirrors it,
`/sys/class/platform-profile/platform-profile-0/` — use that purely as the
udev **trigger**, and have the rule's action relax permission on the actual
path NitroControl uses:

```bash
echo 'SUBSYSTEM=="platform-profile", KERNEL=="platform-profile-0", RUN+="/usr/bin/chgrp wheel /sys/firmware/acpi/platform_profile", RUN+="/usr/bin/chmod 664 /sys/firmware/acpi/platform_profile"' | sudo tee /etc/udev/rules.d/90-nitrocontrol-acer-platform-profile.rules
```

Change `wheel` to whatever group your unprivileged user is actually in if
different (`groups $USER`).

Apply it now (it fires automatically on every future boot on its own — this
manual trigger is only needed once, for the module instance already loaded
before the rule existed):

```bash
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=platform-profile
stat -c '%A %U:%G' /sys/firmware/acpi/platform_profile   # expect: -rw-rw-r-- root:wheel
```

## 3. Confirm

```bash
nitroctl acer-profile set balanced-performance   # no sudo
nitroctl acer-profile get
```

If that works without a password prompt, both steps applied correctly.

## Reverting

```bash
sudo rm /etc/modprobe.d/nitrocontrol-acer-wmi.conf
sudo rm /etc/udev/rules.d/90-nitrocontrol-acer-platform-profile.rules
sudo udevadm control --reload-rules
sudo rmmod acer_wmi && sudo modprobe acer_wmi   # back to predator_v4=N this session
```

(A reboot after removing the two files returns everything to stock, no
manual reload needed.)
