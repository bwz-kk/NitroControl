# NitroControl — CLI (`nitroctl`)

The CLI is the first user-facing interface and must be useful standalone, without the GUI. It only exposes subcommands backed by a capability the detected provider actually verified — see `hardware.md`'s Capability Matrix.

## Commands (v1 target)

```
nitroctl status                  # one-screen summary: CPU/GPU temp+util, RAM, battery, power profile
nitroctl sensors                 # CPU temp, iGPU temp, dGPU temp, CPU freq, CPU util, RAM usage
nitroctl battery                 # percentage, status, power draw (or "unavailable")
nitroctl fans                    # "unavailable" on this hardware — explicit, not omitted
nitroctl profile list            # performance / balanced / power-saver
nitroctl profile get
nitroctl profile set <name>
nitroctl diagnose                # capability matrix + evidence, for GitHub bug reports
```

## Output conventions

- Every metric line names the metric and its unit explicitly (e.g. `CPU temperature: 55.8°C`).
- An unsupported/unknown capability prints its state word, never a fabricated numeric default:
  ```
  Fan RPM: unavailable
  ```
  is correct; `Fan RPM: 0 RPM` is only ever printed if the hardware itself reported `0`.
- `profile set <name>` rejects any value not in the provider's own `list_profiles()` output, with a non-zero exit code and a message naming the valid choices — no silent clamping to a nearby valid value.
- `diagnose` output redacts battery serial number and any hostname/user-identifying DMI field before printing.

## Exit codes

- `0` — success.
- `1` — requested capability is `Unsupported`/`Unknown` for this hardware (not a crash; documented behavior).
- `2` — invalid argument (e.g. unknown profile name).
- `3` — underlying interface call failed (e.g. D-Bus call to `power-profiles-daemon` errored) — the error message names the interface and the underlying error, per SAFE-004 (no silent fallback).

## Testing

- Command-level tests run against a mocked provider (see `architecture.md`'s testing seams) so CLI behavior — including all `Unsupported`/error paths — is verifiable without real hardware.
- One real-hardware run of every command is recorded (output + timestamp) in this repo's test log before a command is documented as working, per the project's Verification requirements.
