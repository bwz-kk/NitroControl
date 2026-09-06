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
nitroctl power-draw               # CPU package power, watts (M10, FR-010) -- root by default, see docs/optional-setup.md
nitroctl diagnose                # capability matrix + evidence, for GitHub bug reports
```

(`acer-profile`, `battery-limit`, `battery-calibrate` — M5/M6/M7's post-v1 additions — aren't listed in this v1-target block; see `roadmap.md`'s milestone sections and `main.rs`'s `Command` enum for their exact subcommand shapes.)

## Output conventions

- Every metric line names the metric and its unit explicitly (e.g. `CPU temperature: 55.8°C`).
- An unsupported/unknown capability prints its state word, never a fabricated numeric default:
  ```
  Fan RPM: unavailable
  ```
  is correct; `Fan RPM: 0 RPM` is only ever printed if the hardware itself reported `0`.
- `RequiresPrivilege` and `HardwareDependent` render as their own words (`requires elevated privilege`, `hardware-dependent`) and — like `Unsupported`/`Unknown` — are not fabricated values; a single-capability command exits `1` for any of the four non-`Supported` states alike, since none of them means "the command ran successfully with a real value."
- `profile set <name>` rejects any value not in the provider's own `list_profiles()` output, with a non-zero exit code and a message naming the valid choices — no silent clamping to a nearby valid value.
- `diagnose` output redacts battery serial number and any hostname/user-identifying DMI field before printing. **Gap closed (M8)**: each metric line is now followed by an indented `  evidence: <source> = <raw value>` line giving the exact sysfs path or subprocess command NitroControl read, and the raw value it got back — present whenever there was a path/command to point at (including `Unknown`-state garbage readings, useful for a bug report), omitted only when the metric is `Unsupported` (nothing found). Redaction is applied by `nitroctl-core::evidence::redact_evidence` before the value ever reaches the CLI: any `KEY=value` line whose key ends in `SERIAL_NUMBER`, `_SERIAL`, `_UUID`, or `_ASSET_TAG` is replaced with `KEY=[REDACTED]` — covers the battery's real `POWER_SUPPLY_SERIAL_NUMBER` field today, and defensively covers any DMI serial/UUID field a future evidence source might read, without needing this function updated first.

## Exit codes

- `0` — success.
- `1` — requested capability is `Unsupported`/`Unknown` for this hardware (not a crash; documented behavior). Applies to single-capability commands (`battery`, `fans`) whose entire output is that one capability. Multi-metric commands (`status`, `sensors`, `diagnose`) always exit `0`: a mix of available/unavailable metrics in one report is normal, documented output, not a command failure — each line still states its own status explicitly.
- `2` — invalid argument (e.g. unknown profile name).
- `3` — underlying interface call failed (e.g. D-Bus call to `power-profiles-daemon` errored) — the error message names the interface and the underlying error, per SAFE-004 (no silent fallback). Not yet reachable in M2 (no command performs a write); becomes relevant with `profile set` in M3.

## CPU utilization sampling

`cpu_utilization`'s rate calculation needs two `/proc/stat` reads (see architecture.md) — but each `nitroctl` invocation is a fresh process with no prior sample. `status`, `sensors`, and `diagnose` handle this by taking a throwaway first sample and, if it isn't `Supported`, sleeping ~200ms and sampling again. This is a short, bounded, documented pause (NFR-002) — not a hang — so real-world CPU utilization is usable from a one-shot CLI command instead of always reading "unknown".

## Testing

- Command-level tests run against a mocked provider (see `architecture.md`'s testing seams) so CLI behavior — including all `Unsupported`/error paths — is verifiable without real hardware.
- One real-hardware run of every command is recorded (output + timestamp) in this repo's test log before a command is documented as working, per the project's Verification requirements.
