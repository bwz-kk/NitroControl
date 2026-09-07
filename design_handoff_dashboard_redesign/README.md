# Handoff: NitroControl dashboard redesign (GTK4 + libadwaita)

## Overview

A redesign of the `nitroctl-gui` dashboard window: a 900×640 landscape window with
tabs on top, four headline metric cards (ring gauge + big value + per-metric live
graph), and a stats strip. Replaces the current 400×760 portrait
`AdwPreferencesPage` + `AdwViewSwitcherBar` layout.

Source of truth for the current UI: `nitroctl-gui/src/window.rs`,
`nitroctl-gui/src/format.rs`, `nitroctl-gui/src/main.rs`.

## About the design files

`NitroControl Redesign.dc.html` and `NitroControl Current.dc.html` in this bundle are
**design references created in HTML** — prototypes showing intended look and behavior,
not production code to port. The task is to recreate the redesign in the existing
GTK4 + libadwaita Rust app using its established patterns: `adw::*` widgets where a
stock widget fits, `gtk4::DrawingArea` + Cairo for the gauges and graphs (same
approach `Sparkline`/`Gauge` already use), and one `gtk4::CssProvider` in
`main.rs::apply_style` for colors — no new crates.

`NitroControl Current.dc.html` is a faithful recreation of today's UI, included only
as a before/after reference.

## Fidelity

**High-fidelity.** Colors, type sizes, spacing, radii and interactions are final.
Recreate pixel-perfectly where GTK allows; where a stock Adwaita widget's own metrics
differ slightly (row heights, switch size), prefer the stock widget.

## Screens / views

All five views live in one `adw::ViewStack`; the switcher moves from the bottom bar to
a **top** bar.

### Window shell

- `adw::ApplicationWindow`, `default_width(900)`, `default_height(640)` (was 400×760).
- `adw::ToolbarView`:
  - top bar 1 — `adw::HeaderBar` (56px): brand row on the left in `set_title_widget`
    or a `pack_start` box — a 3×20px accent bar (rounded 2px, accent glow) +
    "NitroControl" at 17px, weight 500, letter-spacing −0.01em; then a pill
    (1px `#3f424d` border, radius 999px, 12px text `#b2b6ca`) containing a 5px accent
    dot + the DMI product name from `nitroctl_core::dmi` (e.g. `Nitro ANV15-41`).
    `pack_end`: "polling 2s" at 12px `#9397ab`, then the window close button.
  - top bar 2 — `adw::ViewSwitcher` (`Policy::Wide`) or `adw::InlineViewSwitcher`,
    42px tall, bottom hairline `rgba(233,233,237,0.16)`. Active tab: 2px accent
    underline, label `#e9e9ed`; inactive: `#9397ab`, no underline. Labels
    "Overview", "CPU / GPU", "Battery", "Power", "System". Delete
    `ViewSwitcherBar` + `add_bottom_bar`.
  - content — `adw::ToastOverlay` → `adw::ViewStack` (unchanged wiring).
- Content padding 16.8px on all sides; the content area scrolls.

### 1. Overview

- 2×2 grid of metric cards, 16.8px gap, each card `1fr` wide.
  Card: background `#232532`, radius 14px, 1px `#3f424d` edge, padding 11.2px
  11.2px 0. Inside, a horizontal box (gap 11.2px):
  - **Ring gauge**, 60×60 `DrawingArea`: r=24, stroke 5px, round caps, 270° sweep
    starting at 135° — identical geometry to the existing `Gauge`, just smaller
    (`GAUGE_DIAMETER` 60, `GAUGE_ARC_WIDTH` 5.0) and with **no text inside**
    (the number moved out of the ring). Track `#3f424d`; value arc accent.
  - Text column: kicker 12px uppercase, letter-spacing 0.08em, `#9397ab`;
    value 32px weight 500 letter-spacing −0.02em with the unit at 15px `#b2b6ca`;
    meta line 12px `#9397ab`.
  - **Per-metric graph**, full-bleed at the card's bottom: 46px tall `DrawingArea`
    spanning the card's full width (negative 11.2px side margins), 1.5px accent
    stroke + 14%-alpha accent fill under the line. Same `Sparkline` draw code,
    with `SPARKLINE_FILL_ALPHA` 0.14 and one instance per metric.
- Cards, in order: CPU Temperature (`60.8 °C`, meta `1 min · min 54.1 · max 71.6`),
  dGPU Temperature (`47.0 °C`), CPU Utilization (`6.2 %`, meta `1668 MHz`),
  dGPU Utilization (`0.0 %`, meta `idle · iGPU 0.0%`).
- Below: a 3-column strip (`1.4fr 1fr 1fr`, 16.8px gap), same card styling,
  padding 11.2px:
  - RAM Usage — kicker + `50%` right-aligned, `7.5 GiB of 14.9 GiB` at 20px,
    then a 6px progress bar (track `#292b31`, fill accent, radius 3px).
  - CPU Frequency — `1668 MHz` at 20px.
  - Fan RPM — `3032 fan 1` / `2675 fan 2` side by side at 20px, suffixes 12px
    `#9397ab`.

### 2. CPU / GPU

Two labelled sections ("CPU", "GPU" — 13px uppercase 0.08em `#9397ab`), each a
`#232532` card, radius 14px, rows separated by 1px `#292b31`. Row: 8.4px×11.2px
padding, label left, a 200×28 graph in the middle, value right-aligned in an 84px
column at 16px. Rows: CPU Temperature / CPU Utilization / CPU Frequency;
iGPU Temperature / dGPU Temperature / iGPU Utilization / dGPU Utilization.

CPU Package Power is **omitted** — unavailable readings are hidden entirely rather
than dimmed (per the design decision), so drop rows whose `CapabilityState` isn't
`Supported`/`HardwareDependent` instead of adding `dim-label`.

### 3. Battery

- Hero card (padding 16.8px): kicker "BATTERY", `80%` at 42px (unit 20px `#b2b6ca`),
  "Not charging · drawing 0.0 W" at 13px `#b2b6ca`, then a 6px accent progress bar.
- Card with two rows:
  - Battery Charge Limit — subtitle "Caps charging in firmware to slow battery wear"
    (13px `#9397ab`) + the switch. Keep `adw::SwitchRow` semantics and the existing
    poll guard; the mock's switch is 44×24 with an 18px knob (Adwaita's own size is
    fine).
  - Battery Calibration Mode — subtitle mentioning `nitroctl battery-calibrate`
    (inline code in `#7ce7ef`), state shown as an outlined pill (`off`), read-only.

### 4. Power

- "Power Profile": three selectable cards in a 3-column grid, gap 8.4px, radius 8px,
  padding 11.2px. Each: name at 15px + one-line description at 12px `#9397ab`
  ("Quiet, longest runtime" / "Default for daily use" / "Full clocks, loud fans").
  Selected: 1px accent border + accent fill at 12% alpha + 1px accent outer ring.
  Unselected: 1px `#3f424d` border on `#232532`. This replaces the linked
  `ToggleButton` pill row (`ProfilePills`); keep its `set_profile` + toast logic and
  its `applying_from_poll` guard, just re-skin the buttons (a `GtkToggleButton` with
  a vertical box child, `.card`-ish CSS, `:checked` styled by the provider).
- Footnote "via power-profiles-daemon", 12px `#9397ab`.
- "Acer Firmware Profile": one card row with the description
  "Platform profile exposed by the Acer WMI driver" and an outlined accent dropdown
  showing `balanced` — keep `adw::ComboRow`'s behavior; the outlined-accent look is
  CSS on the row's combo button.

### 5. System

- RAM Usage hero card (value 25px, 6px accent bar).
- Two cards side by side: "FAN 1" `3032 RPM`, "FAN 2" `2675 RPM` (value 25px, unit
  14px). Labels stay neutral — `format::fan_rpms` joins an unnamed `Vec<Rpm>`, so
  don't attribute fans to CPU/GPU.
- A footer row (1px `#3f424d` border, radius 14px): "Machine" left,
  `Nitro ANV15-41 · AcerNitroV15 provider` right in `#cfd3e5`.

## Interactions & behavior

- Tab switching: `ViewStack` as today. No animation beyond Adwaita's own crossfade.
- Profile cards / firmware dropdown / charge-limit switch: unchanged provider calls
  (`set_profile`, `set_health_mode`) on `gio::spawn_blocking`, failures surfaced via
  `adw::Toast`, state re-synced by the next 2s poll. Keep the `applying_from_poll`
  guards.
- Hover: interactive elements tint toward the accent
  (`color-mix(accent 14%, transparent)` for outlined/ghost items, `#292b31` for the
  close button). Pressed: one ramp step lighter (`#33dae4`).
- Focus: `outline: 2px solid <accent>; outline-offset: 2px` — no default focus ring.
- Graphs keep 30 samples at the 2s poll (1 minute of history), right-aligned so the
  newest sample sits at the right edge; the min/max in the CPU-temp meta line come
  from that same buffer.
- Unavailable metrics: hide the row/card, don't dim it.

## State management

No new state. Existing `Snapshot` fields cover everything shown, plus:

- one history buffer per graphed metric (currently only CPU temperature has one) —
  generalize `Sparkline` to `Vec<Sparkline>`/one per row and push from
  `Dashboard::apply`;
- min/max for the CPU-temp meta line, derived from that buffer (no new provider call);
- `dmi::detect_provider_kind` + the DMI `product_name` string, read once at startup
  for the header pill and the System footer. `dmi.rs` currently reads `product_name`
  but doesn't expose it — either return it from a small helper or re-read
  `/sys/class/dmi/id/product_name` in the GUI.

Everything shown must come from a real `CapabilityState`; never fabricate a value
(`format.rs` module doc, SAFE-004).

## Design tokens

Accent — the app's existing forced cyan, so `main.rs::apply_style` keeps working:

| Role | Value |
| --- | --- |
| accent | `#00d4e0` |
| accent bg (filled/`:checked`) | `#00b8c4` |
| accent fg (on filled) | `#00272c` |
| accent 100 / 200 / 300 / 400 | `#e6fdfe` / `#b3f3f7` / `#7ce7ef` / `#33dae4` |

Surfaces and text:

| Role | Value |
| --- | --- |
| window ground | `#161826` |
| card surface | `#232532` |
| header gradient | `#1d2032` → `#161826` |
| text | `#e9e9ed` |
| muted text | `#9397ab` (`neutral-500`), secondary `#b2b6ca` (`neutral-400`) |
| borders / gauge track | `#3f424d` (`neutral-800`) |
| row separators / bar tracks | `#292b31` (`neutral-900`) |
| divider hairline | `rgba(233,233,237,0.16)` |
| elevation | 1px edge `#3f424d` + `0 6px 18px rgba(0,0,0,0.55)` when lifted |

Spacing scale (0.7× density): 2.8 / 5.6 / 8.4 / 11.2 / 16.8 / 22.4 px.
Radii: 4 / 8 / 14 px. Type: Inter (or Adwaita Sans), headings weight 500 — never
heavier. Sizes used: 42 / 32 / 25 / 20 / 17 / 15 / 14 / 13 / 12 px.

Note: the app currently forces dark via `adw::ColorScheme::ForceDark` and only
overrides the accent named colors. This design also changes the window and card
grounds, so extend `apply_style`'s CSS with `window`, `.card`/row background and
border colors from the table above rather than relying on stock Adwaita greys.

## Assets

None. Every mark is drawn with Cairo or is a small inline vector (close ×, chevron).
The graphs and gauges are code, not images. Icons in the current bottom bar
(`view-grid-symbolic` etc.) are no longer needed — the top switcher is text-only.

## Files

- `NitroControl Redesign.dc.html` — the redesign, all five views (click the tabs, the
  profile cards, and the charge-limit switch).
- `NitroControl Current.dc.html` — recreation of today's UI, for comparison.
- `styles.css` — the token stylesheet the design is built from (Nocturne), for exact
  variable values.
