//! Dashboard window construction and the polling loop.
//!
//! Per docs/architecture.md, this file is the only place that touches
//! `nitroctl-core` — every value shown is fetched here and handed to
//! `crate::format` for rendering; no widget code reaches back into
//! `nitroctl-core` on its own.
//!
//! `SensorProvider`/`PowerProfileProvider` are synchronous (blocking file
//! IO / D-Bus calls). Both are built **once**, here, and shared via `Arc`
//! (the traits are `Send + Sync` for exactly this) — not rebuilt every
//! poll: `GenericLinux::cpu_utilization()`'s rate calculation only produces
//! a real value across two calls on the *same* instance, so a fresh
//! provider per tick would read "unknown" forever, not just on the first
//! tick. The one-time construction happens synchronously on the main
//! thread (DMI/D-Bus connect is a bounded, sub-second startup cost, not a
//! per-poll one); every recurring poll's actual sensor/D-Bus reads happen
//! on a `gio::spawn_blocking` worker thread, never the GTK main thread,
//! per NFR-002.
//!
//! M17 layout note: this file implements
//! `design_handoff_dashboard_redesign/README.md`'s dashboard redesign
//! (top tab bar, metric cards with ring gauge + big value + live graph,
//! card-styled Battery/Power/System screens). Per that handoff's explicit
//! design decision, **unavailable readings are hidden entirely, not
//! dimmed** — every row/card's container is shown or hidden based on the
//! metric's own `CapabilityState` each poll tick, rather than the old
//! `dim-label` CSS-class approach M4–M15 used.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk4::{cairo, gio, glib, DrawingArea};
use libadwaita as adw;

use nitroctl_core::battery_calibration::BatteryCalibrationProvider;
use nitroctl_core::battery_limit::{
    AcerWmiBatteryBackend, BatteryLimitError, BatteryLimitProvider,
};
use nitroctl_core::capability::CapabilityState;
use nitroctl_core::command::RealCommandRunner;
use nitroctl_core::dmi;
use nitroctl_core::power_profile::{
    AcerPlatformProfileBackend, FailedBackend, PowerProfileProvider, PowerProfilesDaemon,
    ProfileError, ProfileInfo, ZbusPowerProfilesBackend,
};
use nitroctl_core::sensor::{
    BatteryState, BatteryStatus, Celsius, GpuKind, Megahertz, MemoryUsage, Percent, Rpm,
    SensorProvider,
};
use nitroctl_core::sysfs::RealSysfsReader;

const POLL_INTERVAL: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------
// Spacing scale (design_handoff_dashboard_redesign/README.md's "Design
// tokens" table, 0.7x density) -- literal px, since GTK margins/spacing
// take integer or float pixel values directly.
// ---------------------------------------------------------------------
const SPACE_2: i32 = 6; // 5.6px
const SPACE_3: i32 = 8; // 8.4px
const SPACE_4: i32 = 11; // 11.2px
const SPACE_6: i32 = 17; // 16.8px

/// How many samples every graphed metric keeps — 30 samples at the 2s poll
/// interval above is 1 minute of history, enough to see a real trend
/// without a row/card growing unboundedly. Also the source of the
/// CPU/dGPU-temperature cards' "min ... max ..." meta line.
const SPARKLINE_HISTORY_LEN: usize = 30;
/// The app's own forced accent color (`#00d4e0`, `main.rs::apply_style`) —
/// legible on both the light and dark Adwaita row backgrounds without
/// querying the active theme, and consistent with every other accent use.
const SPARKLINE_LINE_RGB: (f64, f64, f64) = (0.0, 0.831, 0.878);
const SPARKLINE_FILL_ALPHA: f64 = 0.14;

/// A small inline history graph — `gtk4::DrawingArea` + Cairo, no charting
/// dependency. Two shapes are used (per the M17 handoff): a fixed-size
/// inline one (200x28, CPU/GPU tab rows) and a full-bleed one (hexpand,
/// 46px tall, Overview metric cards) — `fill_width` picks which.
struct Sparkline {
    widget: DrawingArea,
    history: Rc<RefCell<VecDeque<f64>>>,
}

impl Sparkline {
    fn new(width: i32, height: i32, fill_width: bool) -> Self {
        let history: Rc<RefCell<VecDeque<f64>>> =
            Rc::new(RefCell::new(VecDeque::with_capacity(SPARKLINE_HISTORY_LEN)));
        let widget = DrawingArea::new();
        if fill_width {
            widget.set_hexpand(true);
            widget.set_content_width(1);
        } else {
            widget.set_content_width(width);
        }
        widget.set_content_height(height);
        widget.set_valign(gtk4::Align::Center);

        let draw_history = history.clone();
        widget.set_draw_func(move |_area, cr, width, height| {
            let history = draw_history.borrow();
            if history.len() < 2 {
                return; // nothing to draw a line between yet
            }

            let min = history.iter().cloned().fold(f64::INFINITY, f64::min);
            let max = history.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            // Flat history (every sample identical) would divide by zero —
            // fall back to a 1-unit range so it draws a flat line instead.
            let range = if (max - min).abs() < f64::EPSILON {
                1.0
            } else {
                max - min
            };

            let w = width as f64;
            let h = height as f64;
            let step = w / (SPARKLINE_HISTORY_LEN.saturating_sub(1)) as f64;
            // Right-align: pad the left edge until the buffer fills up, so
            // the most recent sample always sits at the right edge.
            let offset = (SPARKLINE_HISTORY_LEN - history.len()) as f64 * step;

            let points: Vec<(f64, f64)> = history
                .iter()
                .enumerate()
                .map(|(i, &v)| {
                    let x = offset + i as f64 * step;
                    let y = h - ((v - min) / range) * h;
                    (x, y)
                })
                .collect();

            let (r, g, b) = SPARKLINE_LINE_RGB;
            cr.set_source_rgba(r, g, b, 1.0);
            cr.set_line_width(1.5);
            cr.move_to(points[0].0, points[0].1);
            for &(x, y) in &points[1..] {
                cr.line_to(x, y);
            }
            let _ = cr.stroke_preserve(); // keep the path to close it into a fill shape below

            let last_x = points.last().unwrap().0;
            cr.line_to(last_x, h);
            cr.line_to(points[0].0, h);
            cr.close_path();
            cr.set_source_rgba(r, g, b, SPARKLINE_FILL_ALPHA);
            let _ = cr.fill();
        });

        Self { widget, history }
    }

    fn push(&self, value: f64) {
        let mut history = self.history.borrow_mut();
        if history.len() == SPARKLINE_HISTORY_LEN {
            history.pop_front();
        }
        history.push_back(value);
        drop(history);
        self.widget.queue_draw();
    }

    /// The buffer's own min/max — source for the CPU/dGPU-temperature
    /// cards' "1 min · min ... · max ..." meta line (no new provider call).
    fn min_max(&self) -> Option<(f64, f64)> {
        let history = self.history.borrow();
        if history.is_empty() {
            return None;
        }
        let min = history.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = history.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        Some((min, max))
    }
}

const GAUGE_DIAMETER: i32 = 60;
const GAUGE_ARC_WIDTH: f64 = 5.0;
const GAUGE_TRACK_RGBA: (f64, f64, f64, f64) = (0.247, 0.259, 0.302, 1.0); // #3f424d
/// A 270° sweep with the gap at the bottom — the common "speedometer" gauge
/// convention, matching the design handoff's own ring geometry exactly
/// (`stroke-dasharray` computed against a 270°-of-377° circumference).
const GAUGE_START_ANGLE: f64 = 0.75 * std::f64::consts::PI; // 135°
const GAUGE_SWEEP: f64 = 1.5 * std::f64::consts::PI; // 270°

/// A ring gauge — `gtk4::DrawingArea` + Cairo, same approach as `Sparkline`.
/// M17: ring-only (no text inside, per the handoff — the big value moved
/// out into a sibling `Label` using real Pango text instead of Cairo's toy
/// text API, which reads better and simplifies this type).
struct Gauge {
    widget: DrawingArea,
    value: Rc<Cell<Option<f64>>>,
}

impl Gauge {
    /// `max_value` scales the arc (a reading at or above it draws a full
    /// sweep); `color` tints the value arc, distinguishing gauge kinds
    /// (e.g. temperature vs. utilization) rather than implying a safety
    /// threshold this project hasn't validated (SAFE-004: no fabricated
    /// meaning).
    fn new(max_value: f64, color: (f64, f64, f64)) -> Self {
        let value: Rc<Cell<Option<f64>>> = Rc::new(Cell::new(None));
        let widget = DrawingArea::new();
        widget.set_content_width(GAUGE_DIAMETER);
        widget.set_content_height(GAUGE_DIAMETER);

        let draw_value = value.clone();
        widget.set_draw_func(move |_area, cr, width, height| {
            let cx = width as f64 / 2.0;
            let cy = height as f64 / 2.0;
            let radius = (width.min(height) as f64 / 2.0) - GAUGE_ARC_WIDTH;

            cr.set_line_width(GAUGE_ARC_WIDTH);
            cr.set_line_cap(cairo::LineCap::Round);

            let (tr, tg, tb, ta) = GAUGE_TRACK_RGBA;
            cr.set_source_rgba(tr, tg, tb, ta);
            cr.arc(
                cx,
                cy,
                radius,
                GAUGE_START_ANGLE,
                GAUGE_START_ANGLE + GAUGE_SWEEP,
            );
            let _ = cr.stroke();

            if let Some(v) = draw_value.get() {
                let (r, g, b) = color;
                let fraction = (v / max_value).clamp(0.0, 1.0);
                cr.set_source_rgba(r, g, b, 1.0);
                cr.arc(
                    cx,
                    cy,
                    radius,
                    GAUGE_START_ANGLE,
                    GAUGE_START_ANGLE + GAUGE_SWEEP * fraction,
                );
                let _ = cr.stroke();
            }
        });

        Self { widget, value }
    }

    fn set_value(&self, v: Option<f64>) {
        self.value.set(v);
        self.widget.queue_draw();
    }
}

/// Warm orange, used for the two temperature gauges — a plain visual
/// distinction from the utilization gauges' cooler accent, not a
/// fabricated safety threshold (SAFE-004: this project doesn't define
/// one).
const GAUGE_THERMAL_RGB: (f64, f64, f64) = (0.902, 0.494, 0.133);
/// Same forced accent as the sparkline — used for the two utilization
/// gauges.
const GAUGE_ACTIVITY_RGB: (f64, f64, f64) = (0.0, 0.831, 0.878);
/// Laptop CPU/GPU temperatures very rarely exceed 100°C before thermal
/// throttling/shutdown — a natural, intuitive gauge ceiling.
const GAUGE_TEMPERATURE_MAX: f64 = 100.0;
const GAUGE_PERCENT_MAX: f64 = 100.0;

// ---------------------------------------------------------------------
// Raw-value extraction: every `format::*_row` text helper stays for the
// (now hide-vs-show) availability check; these pull the plain f64/etc.
// alongside it for gauges, graphs, bars and meta lines. `None` for every
// non-`Supported`/`HardwareDependent` state — never a fabricated point.
// ---------------------------------------------------------------------

fn celsius_value(state: &CapabilityState<Celsius>) -> Option<f64> {
    match state {
        CapabilityState::Supported(Celsius(v)) | CapabilityState::HardwareDependent(Celsius(v)) => {
            Some(*v)
        }
        _ => None,
    }
}

fn percent_value(state: &CapabilityState<Percent>) -> Option<f64> {
    match state {
        CapabilityState::Supported(Percent(v)) | CapabilityState::HardwareDependent(Percent(v)) => {
            Some(*v)
        }
        _ => None,
    }
}

fn megahertz_value(state: &CapabilityState<Megahertz>) -> Option<f64> {
    match state {
        CapabilityState::Supported(Megahertz(v))
        | CapabilityState::HardwareDependent(Megahertz(v)) => Some(*v),
        _ => None,
    }
}

/// `(used_gib, total_gib, percent)` — the three numbers the RAM cards need.
fn ram_value(state: &CapabilityState<MemoryUsage>) -> Option<(f64, f64, f64)> {
    let usage = match state {
        CapabilityState::Supported(u) | CapabilityState::HardwareDependent(u) => u,
        _ => return None,
    };
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let used = usage.used_bytes as f64 / GIB;
    let total = usage.total_bytes as f64 / GIB;
    let percent = if total > 0.0 {
        used / total * 100.0
    } else {
        0.0
    };
    Some((used, total, percent))
}

/// `(percent, status text, watts)` — what the Battery hero card needs.
/// Status text is a small display mapping over `BatteryStatus`, not its
/// bare `{:?}` (`NotCharging` -> `"Not charging"`), matching the handoff's
/// own wording.
fn battery_value(state: &CapabilityState<BatteryState>) -> Option<(f64, String, Option<f64>)> {
    let b = match state {
        CapabilityState::Supported(b) | CapabilityState::HardwareDependent(b) => b,
        _ => return None,
    };
    let status_text = match b.status {
        BatteryStatus::Charging => "Charging",
        BatteryStatus::Discharging => "Discharging",
        BatteryStatus::Full => "Full",
        BatteryStatus::NotCharging => "Not charging",
        BatteryStatus::Unknown => "Unknown",
    };
    Some((b.percent, status_text.to_string(), b.power_watts))
}

/// Raw fan RPMs — the System tab's "Fan 1"/"Fan 2" cards and Overview's
/// stats-strip need the numbers themselves, not `format::fan_rpm_row`'s
/// joined text. Fans stay unnamed/unordered (per the handoff: don't
/// attribute them to CPU/GPU) — this is just "the Nth reading reported".
fn fan_rpm_values(state: &CapabilityState<Vec<Rpm>>) -> Option<Vec<u32>> {
    match state {
        CapabilityState::Supported(rpms) | CapabilityState::HardwareDependent(rpms) => {
            Some(rpms.iter().map(|r| r.0).collect())
        }
        _ => None,
    }
}

fn build_sensor_provider() -> Arc<dyn SensorProvider> {
    Arc::from(dmi::build_sensor_provider(
        RealSysfsReader,
        RealCommandRunner,
    ))
}

fn build_profile_provider() -> Arc<dyn PowerProfileProvider> {
    match ZbusPowerProfilesBackend::connect() {
        Ok(backend) => Arc::new(PowerProfilesDaemon::new(backend)),
        Err(e) => Arc::new(PowerProfilesDaemon::new(FailedBackend(e))),
    }
}

/// M5/FR-007: a second, independent `PowerProfileProvider` — see
/// docs/architecture.md's M5 design section for why this stays separate
/// from `build_profile_provider()` above rather than merged. No `connect()`
/// step (unlike D-Bus): this is just sysfs reads/writes, so "not available"
/// only shows up per-call (`Unsupported`, the default state, since
/// NitroControl never loads `predator_v4=1` itself).
fn build_acer_profile_provider() -> Arc<dyn PowerProfileProvider> {
    Arc::new(PowerProfilesDaemon::new(AcerPlatformProfileBackend::new(
        RealSysfsReader,
    )))
}

/// M6/FR-008: battery charge limit over the out-of-tree
/// `bwz-kk/acer-wmi-battery` driver's `health_mode` — same shape as
/// `build_acer_profile_provider()` above (no `connect()` step, "not
/// available" per-call when the driver isn't loaded, the default state).
fn build_battery_limit_provider() -> Arc<dyn BatteryLimitProvider> {
    Arc::new(AcerWmiBatteryBackend::new(RealSysfsReader))
}

/// M7/FR-009: battery calibration mode over the same driver's
/// `calibration_mode`. Read-only in this GUI on purpose
/// (docs/architecture.md's M7 design section) — no write control is wired
/// up for this row; `nitroctl battery-calibrate set` stays CLI-only.
fn build_battery_calibration_provider() -> Arc<dyn BatteryCalibrationProvider> {
    Arc::new(AcerWmiBatteryBackend::new(RealSysfsReader))
}

/// Every value the dashboard displays, read in one go on the worker thread.
struct Snapshot {
    cpu_temperature_value: Option<f64>,
    igpu_temperature_value: Option<f64>,
    dgpu_temperature_value: Option<f64>,
    cpu_utilization_value: Option<f64>,
    igpu_utilization_value: Option<f64>,
    dgpu_utilization_value: Option<f64>,
    cpu_frequency_value: Option<f64>,
    ram_value: Option<(f64, f64, f64)>,
    battery_value: Option<(f64, String, Option<f64>)>,
    fan_rpm_values: Option<Vec<u32>>,
    /// M12/M15/M17: `(all profiles with detail, currently active name)`
    /// when both the profile list and the current profile are available —
    /// `None` means nothing real to populate the selector with. Full
    /// `ProfileInfo` (not just names) since issue #25: the picker grays out
    /// a specific known-unsupported entry rather than treating the list as
    /// all-or-nothing.
    power_profile_options: Option<(Vec<ProfileInfo>, String)>,
    acer_profile_options: Option<(Vec<String>, String)>,
    acer_profile_available: bool,
    battery_limit_value: Option<bool>,
    battery_limit_available: bool,
    battery_calibration_value: Option<bool>,
    battery_calibration_available: bool,
}

fn profile_options(provider: &dyn PowerProfileProvider) -> Option<(Vec<String>, String)> {
    let names = match provider.list_profiles() {
        CapabilityState::Supported(names) => names,
        _ => return None,
    };
    let current = match provider.current_profile() {
        CapabilityState::Supported(status) | CapabilityState::HardwareDependent(status) => {
            status.name
        }
        _ => return None,
    };
    Some((names, current))
}

/// Same pairing as `profile_options()`, but with each entry's full detail
/// (`ProfileInfo.known_unsupported`, issue #25) instead of just its name --
/// used for the main Power-tab picker so it can gray out a specific
/// known-bad choice instead of the list being all-or-nothing.
fn profile_options_detailed(
    provider: &dyn PowerProfileProvider,
) -> Option<(Vec<ProfileInfo>, String)> {
    let details = match provider.list_profile_details() {
        CapabilityState::Supported(details) => details,
        _ => return None,
    };
    let current = match provider.current_profile() {
        CapabilityState::Supported(status) | CapabilityState::HardwareDependent(status) => {
            status.name
        }
        _ => return None,
    };
    Some((details, current))
}

fn battery_limit_value(state: &CapabilityState<bool>) -> Option<bool> {
    match state {
        CapabilityState::Supported(v) | CapabilityState::HardwareDependent(v) => Some(*v),
        _ => None,
    }
}

/// Blocking: reads every sensor + both power-profile sources + the battery
/// limit off the shared, long-lived providers. Must only run on a worker
/// thread (`gio::spawn_blocking`), never the GTK main thread.
fn take_snapshot(
    sensors: &dyn SensorProvider,
    profile: &dyn PowerProfileProvider,
    acer_profile: &dyn PowerProfileProvider,
    battery_limit: &dyn BatteryLimitProvider,
    battery_calibration: &dyn BatteryCalibrationProvider,
) -> Snapshot {
    let acer_profile_current = acer_profile.current_profile();
    let battery_limit_state = battery_limit.health_mode();
    let battery_calibration_state = battery_calibration.calibration_mode();
    Snapshot {
        cpu_temperature_value: celsius_value(&sensors.cpu_temperature()),
        igpu_temperature_value: celsius_value(&sensors.gpu_temperature(GpuKind::Integrated)),
        dgpu_temperature_value: celsius_value(&sensors.gpu_temperature(GpuKind::Discrete)),
        cpu_utilization_value: percent_value(&sensors.cpu_utilization()),
        igpu_utilization_value: percent_value(&sensors.gpu_utilization(GpuKind::Integrated)),
        dgpu_utilization_value: percent_value(&sensors.gpu_utilization(GpuKind::Discrete)),
        cpu_frequency_value: megahertz_value(&sensors.cpu_frequency()),
        ram_value: ram_value(&sensors.ram_usage()),
        battery_value: battery_value(&sensors.battery()),
        fan_rpm_values: fan_rpm_values(&sensors.fan_rpm()),
        power_profile_options: profile_options_detailed(profile),
        acer_profile_options: profile_options(acer_profile),
        acer_profile_available: !matches!(
            acer_profile_current,
            CapabilityState::Unsupported | CapabilityState::Unknown
        ),
        battery_limit_value: battery_limit_value(&battery_limit_state),
        battery_limit_available: !matches!(
            battery_limit_state,
            CapabilityState::Unsupported | CapabilityState::Unknown
        ),
        battery_calibration_value: battery_limit_value(&battery_calibration_state),
        battery_calibration_available: !matches!(
            battery_calibration_state,
            CapabilityState::Unsupported | CapabilityState::Unknown
        ),
    }
}

fn profile_error_message(e: &ProfileError) -> String {
    match e {
        ProfileError::InvalidProfile { requested, valid } => format!(
            "Invalid profile {requested:?}; valid choices: {}",
            valid.join(", ")
        ),
        ProfileError::KnownUnsupportedProfile { requested } => format!(
            "{requested:?} is a known, permanent firmware/EC limitation on this hardware -- not a NitroControl bug. See docs/hardware.md's predator_v4 experiment for details."
        ),
        ProfileError::BackendUnavailable => "power-profiles-daemon is not available".to_string(),
        ProfileError::BackendDenied => {
            "Denied -- see docs/optional-setup.md for the udev-rule relax".to_string()
        }
        ProfileError::BackendFailed(msg) => msg.clone(),
    }
}

fn battery_limit_error_message(e: &BatteryLimitError) -> String {
    match e {
        BatteryLimitError::Unavailable => {
            "Driver not loaded -- see docs/optional-setup.md".to_string()
        }
        BatteryLimitError::Denied => {
            "Denied -- needs root or a udev rule, see docs/optional-setup.md".to_string()
        }
        BatteryLimitError::Failed(msg) => msg.clone(),
    }
}

/// "power-saver" -> "Power saver": hyphens to spaces, first letter
/// capitalized, matching the handoff's own profile-card titles.
fn profile_display_name(name: &str) -> String {
    let mut s = name.replace('-', " ");
    if let Some(c) = s.get_mut(0..1) {
        c.make_ascii_uppercase();
    }
    s
}

/// One-line description under each profile card — presentation text only,
/// for the three profile names this hardware is known to report
/// (`hardware.md`'s M3 findings). An unrecognized name (a future PPD
/// backend, a different machine) gets no subtitle rather than a guessed
/// one.
fn profile_subtitle(name: &str) -> &'static str {
    match name {
        "power-saver" => "Quiet, longest runtime",
        "balanced" => "Default for daily use",
        "performance" => "Full clocks, loud fans",
        _ => "",
    }
}

/// An editable "pick one of N named profiles" row — the secondary Acer
/// Firmware Profile on its own Power-tab row. Picking a different entry
/// calls the provider's `set_profile(name)` off the main thread. A failed
/// write is surfaced via an `AdwToast`; no manual revert is needed since
/// the next poll tick (≤2s later) always re-syncs the selection to the
/// real backend state regardless of whether the write succeeded.
struct ProfileRow {
    widget: adw::ComboRow,
    /// Guards against the poll loop's own `set_model`/`set_selected` calls
    /// firing the same `notify::selected` handler a real user click would.
    applying_from_poll: Rc<Cell<bool>>,
    last_names: Rc<RefCell<Vec<String>>>,
}

impl ProfileRow {
    fn new(
        title: &str,
        provider: Arc<dyn PowerProfileProvider>,
        toasts: adw::ToastOverlay,
    ) -> Self {
        let widget = adw::ComboRow::builder().title(title).build();
        widget.add_css_class("accent-combo");
        let applying_from_poll = Rc::new(Cell::new(false));

        let guard = applying_from_poll.clone();
        widget.connect_selected_notify(move |row| {
            if guard.get() {
                return; // our own poll-driven update, not a real user click
            }
            let Some(name) = row
                .selected_item()
                .and_downcast::<gtk4::StringObject>()
                .map(|s| s.string().to_string())
            else {
                return;
            };
            let provider = provider.clone();
            let toasts = toasts.clone();
            glib::MainContext::default().spawn_local(async move {
                let write_name = name.clone();
                let result = gio::spawn_blocking(move || provider.set_profile(&write_name)).await;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => toasts.add_toast(adw::Toast::new(&profile_error_message(&e))),
                    Err(_) => {
                        toasts.add_toast(adw::Toast::new("Couldn't set profile: internal error"))
                    }
                }
            });
        });

        Self {
            widget,
            applying_from_poll,
            last_names: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn apply(&self, options: &Option<(Vec<String>, String)>) {
        self.applying_from_poll.set(true);
        if let Some((names, current)) = options {
            let mut last_names = self.last_names.borrow_mut();
            if *last_names != *names {
                let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                self.widget.set_model(Some(&gtk4::StringList::new(&refs)));
                *last_names = names.clone();
            }
            if let Some(idx) = names.iter().position(|n| n == current) {
                self.widget.set_selected(idx as u32);
            }
        }
        self.applying_from_poll.set(false);
    }
}

/// An editable on/off row — `battery_limit`'s `AdwSwitchRow`. Same poll
/// guard and revert-via-next-poll reasoning as `ProfileRow`.
struct BatteryLimitRow {
    widget: adw::SwitchRow,
    applying_from_poll: Rc<Cell<bool>>,
}

impl BatteryLimitRow {
    fn new(
        title: &str,
        provider: Arc<dyn BatteryLimitProvider>,
        toasts: adw::ToastOverlay,
    ) -> Self {
        let widget = adw::SwitchRow::builder().title(title).build();
        let applying_from_poll = Rc::new(Cell::new(false));

        let guard = applying_from_poll.clone();
        widget.connect_active_notify(move |row| {
            if guard.get() {
                return;
            }
            let enabled = row.is_active();
            let provider = provider.clone();
            let toasts = toasts.clone();
            glib::MainContext::default().spawn_local(async move {
                let result = gio::spawn_blocking(move || provider.set_health_mode(enabled)).await;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        toasts.add_toast(adw::Toast::new(&battery_limit_error_message(&e)))
                    }
                    Err(_) => toasts.add_toast(adw::Toast::new(
                        "Couldn't change battery charge limit: internal error",
                    )),
                }
            });
        });

        Self {
            widget,
            applying_from_poll,
        }
    }

    fn apply(&self, value: Option<bool>) {
        self.applying_from_poll.set(true);
        if let Some(enabled) = value {
            self.widget.set_active(enabled);
        }
        self.applying_from_poll.set(false);
    }
}

/// The Power-tab's "pick a profile" control (M17: re-skinned from M15's
/// linked `ToggleButton` pill row into individual bordered cards, per the
/// design handoff — same interaction model, same poll guard, same
/// `set_profile`/toast write path underneath).
struct ProfilePills {
    container: gtk4::Box,
    buttons: Rc<RefCell<Vec<(String, bool, gtk4::ToggleButton)>>>,
    applying_from_poll: Rc<Cell<bool>>,
    provider: Arc<dyn PowerProfileProvider>,
    toasts: adw::ToastOverlay,
}

impl ProfilePills {
    fn new(provider: Arc<dyn PowerProfileProvider>, toasts: adw::ToastOverlay) -> Self {
        let container = gtk4::Box::new(gtk4::Orientation::Horizontal, SPACE_3);
        container.set_homogeneous(true);

        Self {
            container,
            buttons: Rc::new(RefCell::new(Vec::new())),
            applying_from_poll: Rc::new(Cell::new(false)),
            provider,
            toasts,
        }
    }

    fn make_button(&self, name: &str, known_unsupported: bool) -> gtk4::ToggleButton {
        let button = gtk4::ToggleButton::new();
        button.add_css_class("profile-card");
        button.add_css_class("flat");

        let title = gtk4::Label::new(Some(&profile_display_name(name)));
        title.set_halign(gtk4::Align::Start);
        let mut subtitle_text = profile_subtitle(name).to_string();
        // Issue #25: this exact choice is confirmed in advance to fail on
        // this hardware's firmware/EC -- disable it and say why, instead of
        // letting it be clicked into a raw I/O-error toast.
        if known_unsupported {
            if !subtitle_text.is_empty() {
                subtitle_text.push_str(" -- ");
            }
            subtitle_text.push_str("Not supported by this hardware's firmware");
        }
        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        content.append(&title);
        if !subtitle_text.is_empty() {
            let subtitle = gtk4::Label::new(Some(&subtitle_text));
            subtitle.add_css_class("meta-label");
            subtitle.set_halign(gtk4::Align::Start);
            subtitle.set_wrap(true);
            content.append(&subtitle);
        }
        button.set_child(Some(&content));

        if known_unsupported {
            button.set_sensitive(false);
            button.set_tooltip_text(Some(
                "This profile is a known, permanent firmware/EC limitation on this hardware -- not a NitroControl bug. See docs/hardware.md's predator_v4 experiment for details.",
            ));
        }

        let guard = self.applying_from_poll.clone();
        let provider = self.provider.clone();
        let toasts = self.toasts.clone();
        let write_name = name.to_string();
        button.connect_toggled(move |button| {
            if guard.get() || !button.is_active() {
                return; // poll-driven update, or the button being un-toggled
            }
            let provider = provider.clone();
            let toasts = toasts.clone();
            let write_name = write_name.clone();
            glib::MainContext::default().spawn_local(async move {
                let for_write = write_name.clone();
                let result = gio::spawn_blocking(move || provider.set_profile(&for_write)).await;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => toasts.add_toast(adw::Toast::new(&profile_error_message(&e))),
                    Err(_) => {
                        toasts.add_toast(adw::Toast::new("Couldn't set profile: internal error"))
                    }
                }
            });
        });

        button
    }

    fn apply(&self, options: &Option<(Vec<ProfileInfo>, String)>) {
        self.applying_from_poll.set(true);

        let Some((infos, current)) = options else {
            self.container.set_visible(false);
            self.applying_from_poll.set(false);
            return;
        };
        self.container.set_visible(true);

        let mut buttons = self.buttons.borrow_mut();
        let current_shape: Vec<(String, bool)> = buttons
            .iter()
            .map(|(n, u, _)| (n.clone(), *u))
            .collect();
        let wanted_shape: Vec<(String, bool)> = infos
            .iter()
            .map(|info| (info.name.clone(), info.known_unsupported))
            .collect();
        if current_shape != wanted_shape {
            while let Some(child) = self.container.first_child() {
                self.container.remove(&child);
            }
            let mut new_buttons = Vec::new();
            let mut first_button: Option<gtk4::ToggleButton> = None;
            for info in infos {
                let button = self.make_button(&info.name, info.known_unsupported);
                if let Some(first) = &first_button {
                    button.set_group(Some(first));
                } else {
                    first_button = Some(button.clone());
                }
                self.container.append(&button);
                new_buttons.push((info.name.clone(), info.known_unsupported, button));
            }
            *buttons = new_buttons;
        }
        for (name, _, button) in buttons.iter() {
            if name == current {
                button.set_active(true);
            }
        }
        drop(buttons);

        self.applying_from_poll.set(false);
    }
}

/// One Overview metric card: ring gauge + kicker/value/meta text column +
/// full-bleed live graph underneath. `design_handoff_dashboard_redesign/
/// README.md`'s section 1.
struct MetricCard {
    container: gtk4::Box,
    gauge: Gauge,
    value_label: gtk4::Label,
    meta_label: gtk4::Label,
    sparkline: Sparkline,
    unit: &'static str,
}

impl MetricCard {
    fn new(kicker: &str, unit: &'static str, max_value: f64, color: (f64, f64, f64)) -> Self {
        let gauge = Gauge::new(max_value, color);

        let kicker_label = gtk4::Label::new(Some(&kicker.to_uppercase()));
        kicker_label.add_css_class("kicker-label");
        kicker_label.set_halign(gtk4::Align::Start);

        let value_label = gtk4::Label::new(None);
        value_label.set_halign(gtk4::Align::Start);
        value_label.set_use_markup(true);
        value_label.set_xalign(0.0);

        let meta_label = gtk4::Label::new(None);
        meta_label.add_css_class("meta-label");
        meta_label.set_halign(gtk4::Align::Start);

        let text_column = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        text_column.append(&kicker_label);
        text_column.append(&value_label);
        text_column.append(&meta_label);

        // SPACE_6, not SPACE_4: the handoff's own spec used SPACE_4 here,
        // but that read as too tight against the card edges once rendered
        // in GTK (vs. the mockup's browser rendering) — more inset gives the
        // gauge/text room to breathe and sit more centered in the card.
        let top_row = gtk4::Box::new(gtk4::Orientation::Horizontal, SPACE_4);
        top_row.set_margin_start(SPACE_6);
        top_row.set_margin_end(SPACE_6);
        top_row.set_margin_top(SPACE_6);
        top_row.append(&gauge.widget);
        top_row.append(&text_column);

        let sparkline = Sparkline::new(0, 46, true);
        sparkline.widget.set_margin_top(SPACE_3);

        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        container.add_css_class("card");
        container.set_overflow(gtk4::Overflow::Hidden);
        container.append(&top_row);
        container.append(&sparkline.widget);

        Self {
            container,
            gauge,
            value_label,
            meta_label,
            sparkline,
            unit,
        }
    }

    /// `meta` is pre-formatted by the caller (each card's meta line has
    /// different content — see `MetricCard::apply` call sites in
    /// `build_window`).
    fn apply(&self, value: Option<f64>, meta: &str) {
        self.container.set_visible(value.is_some());
        self.gauge.set_value(value);
        if let Some(v) = value {
            self.value_label.set_markup(&format!(
                "<span size=\"32000\" weight=\"medium\">{v:.1}</span> \
                 <span size=\"15000\" alpha=\"70%\">{}</span>",
                glib::markup_escape_text(self.unit)
            ));
            self.sparkline.push(v);
        }
        self.meta_label.set_text(meta);
        self.meta_label.set_visible(!meta.is_empty());
    }
}

/// One CPU/GPU-tab row: label, inline fixed-size graph, right-aligned
/// value. `design_handoff_dashboard_redesign/README.md`'s section 2.
struct MetricRow {
    container: gtk4::Box,
    value_label: gtk4::Label,
    sparkline: Sparkline,
    unit: &'static str,
    decimals: usize,
}

impl MetricRow {
    /// `decimals` matches the mockup's per-metric convention: temperatures
    /// and percentages show one decimal place, frequency shows none.
    fn new(label: &str, unit: &'static str, decimals: usize) -> Self {
        let label_widget = gtk4::Label::new(Some(label));
        label_widget.set_halign(gtk4::Align::Start);
        label_widget.set_hexpand(true);

        let sparkline = Sparkline::new(200, 28, false);

        let value_label = gtk4::Label::new(None);
        value_label.set_width_chars(9);
        value_label.set_xalign(1.0);
        value_label.add_css_class("title-4");

        let container = gtk4::Box::new(gtk4::Orientation::Horizontal, SPACE_6);
        container.set_margin_start(SPACE_4);
        container.set_margin_end(SPACE_4);
        container.set_margin_top(SPACE_3);
        container.set_margin_bottom(SPACE_3);
        container.add_css_class("metric-row");
        container.append(&label_widget);
        container.append(&sparkline.widget);
        container.append(&value_label);

        Self {
            container,
            value_label,
            sparkline,
            unit,
            decimals,
        }
    }

    fn apply(&self, value: Option<f64>) {
        self.container.set_visible(value.is_some());
        if let Some(v) = value {
            self.value_label
                .set_text(&format!("{v:.*} {}", self.decimals, self.unit));
            self.sparkline.push(v);
        }
    }
}

/// A `.card`-styled section with a title and a list of `MetricRow`s, rows
/// separated by hairlines (CSS `.metric-row` + `:not(:last-child)`).
fn metric_section(title: &str, rows: &[&MetricRow]) -> gtk4::Box {
    let heading = gtk4::Label::new(Some(&title.to_uppercase()));
    heading.add_css_class("kicker-label");
    heading.set_halign(gtk4::Align::Start);
    heading.set_margin_bottom(SPACE_3);

    let card = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    card.add_css_class("card");
    card.set_overflow(gtk4::Overflow::Hidden);
    for row in rows {
        card.append(&row.container);
    }

    let section = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    section.append(&heading);
    section.append(&card);
    section
}

/// A small `.card` stat tile — RAM/CPU-frequency/Fan-RPM on Overview and
/// System, all built from this one shape.
struct StatCard {
    container: gtk4::Box,
    body: gtk4::Box,
}

impl StatCard {
    fn new(kicker: &str) -> Self {
        let kicker_label = gtk4::Label::new(Some(&kicker.to_uppercase()));
        kicker_label.add_css_class("kicker-label");
        kicker_label.set_halign(gtk4::Align::Start);

        let body = gtk4::Box::new(gtk4::Orientation::Vertical, 2);

        // `margin` on a widget pushes it away from its *own* parent — it
        // doesn't pad that widget's children. So the SPACE_6 inset has to
        // live on this inner box (a child of `container`), not on
        // `container` itself; margining `container` would just add gap
        // *outside* the card, between it and its Grid cell, while the
        // kicker/value text inside would still sit flush against the
        // card's own edges. Same fix as `MetricCard`'s `top_row`.
        let inner = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        inner.set_margin_start(SPACE_6);
        inner.set_margin_end(SPACE_6);
        inner.set_margin_top(SPACE_6);
        inner.set_margin_bottom(SPACE_6);
        inner.append(&kicker_label);
        inner.append(&body);

        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        container.add_css_class("card");
        // Defense-in-depth, matching `MetricCard`: if a value ever renders
        // wider than the card gets allocated (narrow window, long text),
        // clip to the card's rounded shape rather than spilling past its
        // border.
        container.set_overflow(gtk4::Overflow::Hidden);
        container.append(&inner);

        Self { container, body }
    }
}

/// The RAM Usage stat: kicker + badge, big value, progress bar — used on
/// both Overview (compact) and System (larger).
struct RamCard {
    container: gtk4::Box,
    badge: gtk4::Label,
    value_label: gtk4::Label,
    bar: gtk4::ProgressBar,
}

impl RamCard {
    fn new(value_size: &str) -> Self {
        let kicker_label = gtk4::Label::new(Some("RAM USAGE"));
        kicker_label.add_css_class("kicker-label");
        kicker_label.set_halign(gtk4::Align::Start);
        kicker_label.set_hexpand(true);

        let badge = gtk4::Label::new(None);
        badge.add_css_class("meta-label");

        let header = gtk4::Box::new(gtk4::Orientation::Horizontal, SPACE_2);
        header.append(&kicker_label);
        header.append(&badge);

        let value_label = gtk4::Label::new(None);
        value_label.set_halign(gtk4::Align::Start);
        value_label.set_use_markup(true);
        value_label.add_css_class(value_size);
        value_label.set_margin_top(2);
        value_label.set_margin_bottom(SPACE_3);

        let bar = gtk4::ProgressBar::new();
        bar.set_hexpand(true);

        // See the same note in `StatCard::new`: the inset has to live on
        // this inner box, not on `container`, or it just adds gap outside
        // the card instead of padding its content.
        let inner = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        inner.set_margin_start(SPACE_6);
        inner.set_margin_end(SPACE_6);
        inner.set_margin_top(SPACE_6);
        inner.set_margin_bottom(SPACE_6);
        inner.append(&header);
        inner.append(&value_label);
        inner.append(&bar);

        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        container.add_css_class("card");
        container.set_overflow(gtk4::Overflow::Hidden);
        container.append(&inner);

        Self {
            container,
            badge,
            value_label,
            bar,
        }
    }

    fn apply(&self, value: Option<(f64, f64, f64)>) {
        self.container.set_visible(value.is_some());
        if let Some((used, total, percent)) = value {
            self.badge.set_text(&format!("{percent:.0}%"));
            self.value_label.set_markup(&format!(
                "{used:.1} <span alpha=\"70%\" size=\"smaller\">GiB of {total:.1} GiB</span>"
            ));
            self.bar.set_fraction((percent / 100.0).clamp(0.0, 1.0));
        }
    }
}

struct Dashboard {
    cpu_temperature_card: MetricCard,
    dgpu_temperature_card: MetricCard,
    cpu_utilization_card: MetricCard,
    dgpu_utilization_card: MetricCard,
    overview_ram: RamCard,
    overview_cpu_freq: StatCard,
    overview_cpu_freq_value: gtk4::Label,
    overview_fans: StatCard,
    overview_fan1_value: gtk4::Label,
    overview_fan2_value: gtk4::Label,

    cpu_temperature_row: MetricRow,
    cpu_utilization_row: MetricRow,
    cpu_frequency_row: MetricRow,
    igpu_temperature_row: MetricRow,
    dgpu_temperature_row: MetricRow,
    igpu_utilization_row: MetricRow,
    dgpu_utilization_row: MetricRow,

    battery_hero: gtk4::Box,
    battery_percent_label: gtk4::Label,
    battery_status_label: gtk4::Label,
    battery_bar: gtk4::ProgressBar,
    battery_limit: BatteryLimitRow,
    battery_limit_row: gtk4::Widget,
    battery_calibration_row: gtk4::Box,
    battery_calibration_pill: gtk4::Label,

    power_profile_pills: ProfilePills,
    acer_profile: ProfileRow,
    acer_profile_row: gtk4::Widget,

    system_ram: RamCard,
    system_fan1: StatCard,
    system_fan1_value: gtk4::Label,
    system_fan2: StatCard,
    system_fan2_value: gtk4::Label,
}

impl Dashboard {
    fn apply(&self, snapshot: &Snapshot) {
        self.cpu_temperature_card.apply(
            snapshot.cpu_temperature_value,
            &self
                .cpu_temperature_card
                .sparkline
                .min_max()
                .map(|(min, max)| format!("1 min · min {min:.1} · max {max:.1}"))
                .unwrap_or_default(),
        );
        self.dgpu_temperature_card.apply(
            snapshot.dgpu_temperature_value,
            &self
                .dgpu_temperature_card
                .sparkline
                .min_max()
                .map(|(min, max)| format!("1 min · min {min:.1} · max {max:.1}"))
                .unwrap_or_default(),
        );
        self.cpu_utilization_card.apply(
            snapshot.cpu_utilization_value,
            &snapshot
                .cpu_frequency_value
                .map(|f| format!("{f:.0} MHz"))
                .unwrap_or_default(),
        );
        let dgpu_util_meta = match (
            snapshot.dgpu_utilization_value,
            snapshot.igpu_utilization_value,
        ) {
            (Some(dgpu), Some(igpu)) if dgpu < 0.5 => format!("idle · iGPU {igpu:.1}%"),
            (Some(_), Some(igpu)) => format!("iGPU {igpu:.1}%"),
            (Some(dgpu), None) if dgpu < 0.5 => "idle".to_string(),
            _ => String::new(),
        };
        self.dgpu_utilization_card
            .apply(snapshot.dgpu_utilization_value, &dgpu_util_meta);

        self.overview_ram.apply(snapshot.ram_value);
        self.overview_cpu_freq
            .container
            .set_visible(snapshot.cpu_frequency_value.is_some());
        if let Some(f) = snapshot.cpu_frequency_value {
            self.overview_cpu_freq_value
                .set_markup(&format!("{f:.0} <span alpha=\"70%\">MHz</span>"));
        }
        let fans_available = snapshot.fan_rpm_values.as_deref().unwrap_or(&[]);
        self.overview_fans
            .container
            .set_visible(!fans_available.is_empty());
        self.overview_fan1_value
            .set_visible(!fans_available.is_empty());
        self.overview_fan2_value
            .set_visible(fans_available.len() > 1);
        if let Some(rpm) = fans_available.first() {
            self.overview_fan1_value.set_markup(&format!(
                "{rpm} <span alpha=\"70%\" size=\"smaller\">fan 1</span>"
            ));
        }
        if let Some(rpm) = fans_available.get(1) {
            self.overview_fan2_value.set_markup(&format!(
                "{rpm} <span alpha=\"70%\" size=\"smaller\">fan 2</span>"
            ));
        }

        self.cpu_temperature_row
            .apply(snapshot.cpu_temperature_value);
        self.cpu_utilization_row
            .apply(snapshot.cpu_utilization_value);
        self.cpu_frequency_row.apply(snapshot.cpu_frequency_value);
        self.igpu_temperature_row
            .apply(snapshot.igpu_temperature_value);
        self.dgpu_temperature_row
            .apply(snapshot.dgpu_temperature_value);
        self.igpu_utilization_row
            .apply(snapshot.igpu_utilization_value);
        self.dgpu_utilization_row
            .apply(snapshot.dgpu_utilization_value);

        self.battery_hero
            .set_visible(snapshot.battery_value.is_some());
        if let Some((percent, status, watts)) = &snapshot.battery_value {
            self.battery_percent_label.set_markup(&format!(
                "{percent:.0}<span size=\"smaller\" alpha=\"70%\">%</span>"
            ));
            let power_text = match watts {
                Some(w) => format!("{status} · drawing {w:.1} W"),
                None => status.clone(),
            };
            self.battery_status_label.set_text(&power_text);
            self.battery_bar
                .set_fraction((percent / 100.0).clamp(0.0, 1.0));
        }

        self.battery_limit_row
            .set_visible(snapshot.battery_limit_available);
        self.battery_limit.apply(snapshot.battery_limit_value);
        self.battery_calibration_row
            .set_visible(snapshot.battery_calibration_available);
        if let Some(on) = snapshot.battery_calibration_value {
            self.battery_calibration_pill
                .set_text(if on { "on" } else { "off" });
        }

        self.power_profile_pills
            .apply(&snapshot.power_profile_options);
        self.acer_profile_row
            .set_visible(snapshot.acer_profile_available);
        self.acer_profile.apply(&snapshot.acer_profile_options);

        self.system_ram.apply(snapshot.ram_value);
        self.system_fan1
            .container
            .set_visible(!fans_available.is_empty());
        self.system_fan2
            .container
            .set_visible(fans_available.len() > 1);
        if let Some(rpm) = fans_available.first() {
            self.system_fan1_value
                .set_markup(&format!("{rpm} <span alpha=\"70%\">RPM</span>"));
        }
        if let Some(rpm) = fans_available.get(1) {
            self.system_fan2_value
                .set_markup(&format!("{rpm} <span alpha=\"70%\">RPM</span>"));
        }
    }
}

fn tab_button(label: &str, active: bool) -> gtk4::ToggleButton {
    let button = gtk4::ToggleButton::builder().label(label).build();
    button.set_active(active);
    button.add_css_class("tab-btn");
    button.add_css_class("flat");
    button
}

pub fn build_window(app: &adw::Application) -> adw::ApplicationWindow {
    let toast_overlay = adw::ToastOverlay::new();
    let profile_provider = build_profile_provider();
    let acer_profile_provider = build_acer_profile_provider();
    let battery_limit_provider = build_battery_limit_provider();

    // ---- Header: brand + DMI pill + polling indicator ----
    let accent_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    accent_bar.set_size_request(3, 20);
    accent_bar.add_css_class("accent-bar");
    let title_label = gtk4::Label::new(Some("NitroControl"));
    title_label.add_css_class("title-4");
    let brand_box = gtk4::Box::new(gtk4::Orientation::Horizontal, SPACE_3);
    brand_box.append(&accent_bar);
    brand_box.append(&title_label);

    let title_widget_box = gtk4::Box::new(gtk4::Orientation::Horizontal, SPACE_3);
    title_widget_box.append(&brand_box);
    if let Some(name) = dmi::product_name(&RealSysfsReader) {
        let dot = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        dot.set_size_request(5, 5);
        dot.add_css_class("accent-dot");
        let name_label = gtk4::Label::new(Some(&name));
        let pill = gtk4::Box::new(gtk4::Orientation::Horizontal, SPACE_2);
        pill.add_css_class("pill");
        pill.append(&dot);
        pill.append(&name_label);
        title_widget_box.append(&pill);
    }

    let header_bar = adw::HeaderBar::new();
    header_bar.set_title_widget(Some(&title_widget_box));
    let polling_label = gtk4::Label::new(Some("polling 2s"));
    polling_label.add_css_class("meta-label");
    header_bar.pack_end(&polling_label);

    // ---- Top tab bar (replaces the M15 bottom AdwViewSwitcherBar) ----
    let view_stack = adw::ViewStack::new();
    let tabs = [
        ("overview", "Overview"),
        ("cpu-gpu", "CPU / GPU"),
        ("battery", "Battery"),
        ("power", "Power"),
        ("system", "System"),
    ];
    let tab_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, SPACE_2);
    tab_bar.add_css_class("tab-bar");
    let mut first_tab: Option<gtk4::ToggleButton> = None;
    for (i, (name, label)) in tabs.iter().enumerate() {
        let button = tab_button(label, i == 0);
        if let Some(first) = &first_tab {
            button.set_group(Some(first));
        } else {
            first_tab = Some(button.clone());
        }
        let stack = view_stack.clone();
        let name = name.to_string();
        button.connect_toggled(move |b| {
            if b.is_active() {
                stack.set_visible_child_name(&name);
            }
        });
        tab_bar.append(&button);
    }

    // ---- Overview tab ----
    let cpu_temperature_card = MetricCard::new(
        "CPU Temperature",
        "°C",
        GAUGE_TEMPERATURE_MAX,
        GAUGE_THERMAL_RGB,
    );
    let dgpu_temperature_card = MetricCard::new(
        "dGPU Temperature",
        "°C",
        GAUGE_TEMPERATURE_MAX,
        GAUGE_THERMAL_RGB,
    );
    let cpu_utilization_card = MetricCard::new(
        "CPU Utilization",
        "%",
        GAUGE_PERCENT_MAX,
        GAUGE_ACTIVITY_RGB,
    );
    let dgpu_utilization_card = MetricCard::new(
        "dGPU Utilization",
        "%",
        GAUGE_PERCENT_MAX,
        GAUGE_ACTIVITY_RGB,
    );

    let overview_grid = gtk4::Grid::builder()
        .row_spacing(SPACE_4)
        .column_spacing(SPACE_4)
        .column_homogeneous(true)
        .build();
    overview_grid.attach(&cpu_temperature_card.container, 0, 0, 1, 1);
    overview_grid.attach(&dgpu_temperature_card.container, 1, 0, 1, 1);
    overview_grid.attach(&cpu_utilization_card.container, 0, 1, 1, 1);
    overview_grid.attach(&dgpu_utilization_card.container, 1, 1, 1, 1);

    let overview_ram = RamCard::new("title-3");
    let overview_cpu_freq = StatCard::new("CPU Frequency");
    let overview_cpu_freq_value = gtk4::Label::new(None);
    overview_cpu_freq_value.set_halign(gtk4::Align::Start);
    overview_cpu_freq_value.set_use_markup(true);
    overview_cpu_freq_value.add_css_class("title-3");
    overview_cpu_freq.body.append(&overview_cpu_freq_value);

    // Stacked, not side-by-side: at the window's default width, this card
    // only gets a quarter of the overview strip's width (see
    // `overview_strip` below), and two `title-3`-sized values in a row
    // don't fit — the second one overflowed past the card's own border.
    // One value per line always fits, matching the CPU Frequency card's
    // single-line convention.
    let overview_fans = StatCard::new("Fan RPM");
    let fan_values_row = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    let overview_fan1_value = gtk4::Label::new(None);
    overview_fan1_value.set_use_markup(true);
    overview_fan1_value.set_halign(gtk4::Align::Start);
    overview_fan1_value.add_css_class("title-3");
    let overview_fan2_value = gtk4::Label::new(None);
    overview_fan2_value.set_use_markup(true);
    overview_fan2_value.set_halign(gtk4::Align::Start);
    overview_fan2_value.add_css_class("title-3");
    fan_values_row.append(&overview_fan1_value);
    fan_values_row.append(&overview_fan2_value);
    overview_fans.body.append(&fan_values_row);

    let overview_strip = gtk4::Grid::builder()
        .row_spacing(SPACE_4)
        .column_spacing(SPACE_4)
        .column_homogeneous(true)
        .build();
    overview_strip.attach(&overview_ram.container, 0, 0, 2, 1);
    overview_strip.attach(&overview_cpu_freq.container, 2, 0, 1, 1);
    overview_strip.attach(&overview_fans.container, 3, 0, 1, 1);

    let overview_page = gtk4::Box::new(gtk4::Orientation::Vertical, SPACE_4);
    overview_page.set_margin_start(SPACE_6);
    overview_page.set_margin_end(SPACE_6);
    overview_page.set_margin_top(SPACE_6);
    overview_page.set_margin_bottom(SPACE_6);
    overview_page.append(&overview_grid);
    overview_page.append(&overview_strip);
    let overview_scroller = gtk4::ScrolledWindow::builder()
        .child(&overview_page)
        .build();

    // ---- CPU / GPU tab ----
    let cpu_temperature_row = MetricRow::new("CPU Temperature", "°C", 1);
    let cpu_utilization_row = MetricRow::new("CPU Utilization", "%", 1);
    let cpu_frequency_row = MetricRow::new("CPU Frequency", "MHz", 0);
    let igpu_temperature_row = MetricRow::new("iGPU Temperature", "°C", 1);
    let dgpu_temperature_row = MetricRow::new("dGPU Temperature", "°C", 1);
    let igpu_utilization_row = MetricRow::new("iGPU Utilization", "%", 1);
    let dgpu_utilization_row = MetricRow::new("dGPU Utilization", "%", 1);

    let cpu_section = metric_section(
        "CPU",
        &[
            &cpu_temperature_row,
            &cpu_utilization_row,
            &cpu_frequency_row,
        ],
    );
    let gpu_section = metric_section(
        "GPU",
        &[
            &igpu_temperature_row,
            &dgpu_temperature_row,
            &igpu_utilization_row,
            &dgpu_utilization_row,
        ],
    );

    let cpu_gpu_page = gtk4::Box::new(gtk4::Orientation::Vertical, SPACE_6);
    cpu_gpu_page.set_margin_start(SPACE_6);
    cpu_gpu_page.set_margin_end(SPACE_6);
    cpu_gpu_page.set_margin_top(SPACE_6);
    cpu_gpu_page.set_margin_bottom(SPACE_6);
    cpu_gpu_page.append(&cpu_section);
    cpu_gpu_page.append(&gpu_section);
    let cpu_gpu_scroller = gtk4::ScrolledWindow::builder().child(&cpu_gpu_page).build();

    // ---- Battery tab ----
    let battery_kicker = gtk4::Label::new(Some("BATTERY"));
    battery_kicker.add_css_class("kicker-label");
    battery_kicker.set_halign(gtk4::Align::Start);
    let battery_percent_label = gtk4::Label::new(None);
    battery_percent_label.set_use_markup(true);
    battery_percent_label.add_css_class("title-1");
    let battery_status_label = gtk4::Label::new(None);
    battery_status_label.add_css_class("meta-label");
    let battery_top_row = gtk4::Box::new(gtk4::Orientation::Horizontal, SPACE_3);
    battery_top_row.set_valign(gtk4::Align::Baseline);
    battery_top_row.append(&battery_percent_label);
    battery_top_row.append(&battery_status_label);
    battery_top_row.set_margin_bottom(SPACE_4);
    let battery_bar = gtk4::ProgressBar::new();
    // Inset lives on this inner box, not on `battery_hero` itself — margin
    // on a widget pushes it away from its *own* parent, it doesn't pad that
    // widget's children (see the same note on `StatCard`/`RamCard`).
    let battery_hero_inner = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    battery_hero_inner.set_margin_top(SPACE_6);
    battery_hero_inner.set_margin_bottom(SPACE_6);
    battery_hero_inner.set_margin_start(SPACE_6);
    battery_hero_inner.set_margin_end(SPACE_6);
    battery_hero_inner.append(&battery_kicker);
    battery_hero_inner.append(&battery_top_row);
    battery_hero_inner.append(&battery_bar);
    let battery_hero = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    battery_hero.add_css_class("card");
    battery_hero.set_overflow(gtk4::Overflow::Hidden);
    battery_hero.append(&battery_hero_inner);

    let battery_limit = BatteryLimitRow::new(
        "Battery Charge Limit",
        battery_limit_provider.clone(),
        toast_overlay.clone(),
    );
    battery_limit
        .widget
        .set_subtitle("Caps charging in firmware to slow battery wear");
    let battery_limit_row: gtk4::Widget = battery_limit.widget.clone().upcast();

    let battery_calibration_title = gtk4::Label::new(Some("Battery Calibration Mode"));
    battery_calibration_title.set_halign(gtk4::Align::Start);
    let battery_calibration_subtitle =
        gtk4::Label::new(Some("Read-only here · set with nitroctl battery-calibrate"));
    battery_calibration_subtitle.add_css_class("meta-label");
    battery_calibration_subtitle.set_halign(gtk4::Align::Start);
    let battery_calibration_text = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    battery_calibration_text.set_hexpand(true);
    battery_calibration_text.append(&battery_calibration_title);
    battery_calibration_text.append(&battery_calibration_subtitle);
    let battery_calibration_pill = gtk4::Label::new(Some("off"));
    battery_calibration_pill.add_css_class("pill");
    let battery_calibration_row = gtk4::Box::new(gtk4::Orientation::Horizontal, SPACE_4);
    battery_calibration_row.set_margin_start(SPACE_4);
    battery_calibration_row.set_margin_end(SPACE_4);
    battery_calibration_row.set_margin_top(SPACE_4);
    battery_calibration_row.set_margin_bottom(SPACE_4);
    battery_calibration_row.append(&battery_calibration_text);
    battery_calibration_row.append(&battery_calibration_pill);

    let battery_controls_card = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    battery_controls_card.add_css_class("card");
    battery_controls_card.set_overflow(gtk4::Overflow::Hidden);
    battery_controls_card.append(&battery_limit_row);
    battery_controls_card.append(&battery_calibration_row);

    let battery_page = gtk4::Box::new(gtk4::Orientation::Vertical, SPACE_4);
    battery_page.set_margin_bottom(SPACE_6);
    battery_page.append(&battery_hero);
    let battery_controls_wrap = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    battery_controls_wrap.set_margin_start(SPACE_6);
    battery_controls_wrap.set_margin_end(SPACE_6);
    battery_controls_wrap.append(&battery_controls_card);
    battery_page.append(&battery_controls_wrap);
    let battery_scroller = gtk4::ScrolledWindow::builder().child(&battery_page).build();

    // ---- Power tab ----
    let power_kicker = gtk4::Label::new(Some("POWER PROFILE"));
    power_kicker.add_css_class("kicker-label");
    power_kicker.set_halign(gtk4::Align::Start);
    power_kicker.set_margin_bottom(SPACE_3);
    let power_profile_pills = ProfilePills::new(profile_provider.clone(), toast_overlay.clone());
    let power_footnote = gtk4::Label::new(Some("via power-profiles-daemon"));
    power_footnote.add_css_class("meta-label");
    power_footnote.set_halign(gtk4::Align::Start);
    power_footnote.set_margin_top(SPACE_3);

    let acer_kicker = gtk4::Label::new(Some("ACER FIRMWARE PROFILE"));
    acer_kicker.add_css_class("kicker-label");
    acer_kicker.set_halign(gtk4::Align::Start);
    acer_kicker.set_margin_top(SPACE_6);
    acer_kicker.set_margin_bottom(SPACE_3);
    let acer_profile = ProfileRow::new(
        "Acer Firmware Profile",
        acer_profile_provider.clone(),
        toast_overlay.clone(),
    );
    acer_profile
        .widget
        .set_subtitle("Platform profile exposed by the Acer WMI driver");
    let acer_profile_group = adw::PreferencesGroup::new();
    acer_profile_group.add(&acer_profile.widget);
    let acer_profile_row: gtk4::Widget = acer_profile_group.clone().upcast();

    let power_page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    power_page.set_margin_start(SPACE_6);
    power_page.set_margin_end(SPACE_6);
    power_page.set_margin_top(SPACE_6);
    power_page.set_margin_bottom(SPACE_6);
    power_page.append(&power_kicker);
    power_page.append(&power_profile_pills.container);
    power_page.append(&power_footnote);
    power_page.append(&acer_kicker);
    power_page.append(&acer_profile_group);
    let power_scroller = gtk4::ScrolledWindow::builder().child(&power_page).build();

    // ---- System tab ----
    let system_ram = RamCard::new("title-2");
    let system_fan1 = StatCard::new("Fan 1");
    let system_fan1_value = gtk4::Label::new(None);
    system_fan1_value.set_use_markup(true);
    system_fan1_value.add_css_class("title-2");
    system_fan1.body.append(&system_fan1_value);
    let system_fan2 = StatCard::new("Fan 2");
    let system_fan2_value = gtk4::Label::new(None);
    system_fan2_value.set_use_markup(true);
    system_fan2_value.add_css_class("title-2");
    system_fan2.body.append(&system_fan2_value);

    let system_fans_grid = gtk4::Grid::builder()
        .column_spacing(SPACE_4)
        .column_homogeneous(true)
        .build();
    system_fans_grid.attach(&system_fan1.container, 0, 0, 1, 1);
    system_fans_grid.attach(&system_fan2.container, 1, 0, 1, 1);

    let machine_label = gtk4::Label::new(Some("Machine"));
    machine_label.set_halign(gtk4::Align::Start);
    machine_label.set_hexpand(true);
    let machine_value = gtk4::Label::new(Some(&{
        let provider_kind = format!("{:?}", dmi::detect_provider_kind(&RealSysfsReader));
        match dmi::product_name(&RealSysfsReader) {
            Some(name) => format!("{name} · {provider_kind} provider"),
            None => format!("{provider_kind} provider"),
        }
    }));
    machine_value.add_css_class("meta-label");
    let machine_row = gtk4::Box::new(gtk4::Orientation::Horizontal, SPACE_4);
    machine_row.add_css_class("pill");
    machine_row.set_margin_start(SPACE_6);
    machine_row.set_margin_end(SPACE_6);
    machine_row.append(&machine_label);
    machine_row.append(&machine_value);

    let system_page = gtk4::Box::new(gtk4::Orientation::Vertical, SPACE_4);
    system_page.set_margin_top(SPACE_6);
    system_page.set_margin_bottom(SPACE_6);
    let system_ram_wrap = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    system_ram_wrap.set_margin_start(SPACE_6);
    system_ram_wrap.set_margin_end(SPACE_6);
    system_ram_wrap.append(&system_ram.container);
    system_page.append(&system_ram_wrap);
    let system_fans_wrap = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    system_fans_wrap.set_margin_start(SPACE_6);
    system_fans_wrap.set_margin_end(SPACE_6);
    system_fans_wrap.append(&system_fans_grid);
    system_page.append(&system_fans_wrap);
    system_page.append(&machine_row);
    let system_scroller = gtk4::ScrolledWindow::builder().child(&system_page).build();

    // ---- Assemble the stack + toolbar ----
    view_stack.add_titled(&overview_scroller, Some("overview"), "Overview");
    view_stack.add_titled(&cpu_gpu_scroller, Some("cpu-gpu"), "CPU / GPU");
    view_stack.add_titled(&battery_scroller, Some("battery"), "Battery");
    view_stack.add_titled(&power_scroller, Some("power"), "Power");
    view_stack.add_titled(&system_scroller, Some("system"), "System");

    toast_overlay.set_child(Some(&view_stack));

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header_bar);
    toolbar_view.add_top_bar(&tab_bar);
    toolbar_view.set_content(Some(&toast_overlay));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("NitroControl")
        // M17: landscape, per the design handoff (was 400x760 portrait, M13).
        .default_width(900)
        .default_height(640)
        .content(&toolbar_view)
        .build();

    let dashboard = Rc::new(Dashboard {
        cpu_temperature_card,
        dgpu_temperature_card,
        cpu_utilization_card,
        dgpu_utilization_card,
        overview_ram,
        overview_cpu_freq,
        overview_cpu_freq_value,
        overview_fans,
        overview_fan1_value,
        overview_fan2_value,
        cpu_temperature_row,
        cpu_utilization_row,
        cpu_frequency_row,
        igpu_temperature_row,
        dgpu_temperature_row,
        igpu_utilization_row,
        dgpu_utilization_row,
        battery_hero,
        battery_percent_label,
        battery_status_label,
        battery_bar,
        battery_limit,
        battery_limit_row,
        battery_calibration_row,
        battery_calibration_pill,
        power_profile_pills,
        acer_profile,
        acer_profile_row,
        system_ram,
        system_fan1,
        system_fan1_value,
        system_fan2,
        system_fan2_value,
    });

    // Built once, shared across every poll — see the module doc comment
    // for why a fresh provider per tick would break cpu_utilization.
    let sensors = build_sensor_provider();
    let battery_calibration_provider = build_battery_calibration_provider();

    // Guards against overlapping poll ticks: if a snapshot is still running
    // (e.g. a slow D-Bus call) when the next timer tick fires, that tick is
    // skipped rather than spawning a second worker task racing the first.
    let poll_in_flight = Rc::new(Cell::new(false));

    let poll = {
        let dashboard = dashboard.clone();
        let poll_in_flight = poll_in_flight.clone();
        move || {
            if poll_in_flight.get() {
                return;
            }
            poll_in_flight.set(true);

            let dashboard = dashboard.clone();
            let sensors = sensors.clone();
            let profile_provider = profile_provider.clone();
            let acer_profile_provider = acer_profile_provider.clone();
            let battery_limit_provider = battery_limit_provider.clone();
            let battery_calibration_provider = battery_calibration_provider.clone();
            let poll_in_flight = poll_in_flight.clone();
            glib::MainContext::default().spawn_local(async move {
                let result = gio::spawn_blocking(move || {
                    take_snapshot(
                        sensors.as_ref(),
                        profile_provider.as_ref(),
                        acer_profile_provider.as_ref(),
                        battery_limit_provider.as_ref(),
                        battery_calibration_provider.as_ref(),
                    )
                })
                .await;
                poll_in_flight.set(false);
                match result {
                    Ok(snapshot) => dashboard.apply(&snapshot),
                    Err(e) => {
                        // Worker thread panicked (or the task was cancelled).
                        // Never crash the GUI over a single bad poll tick —
                        // just skip this update and try again next tick.
                        eprintln!("nitroctl-gui: snapshot poll failed: {e:?}");
                    }
                }
            });
        }
    };

    poll(); // first paint, off the main thread like every later tick
    glib::timeout_add_local(POLL_INTERVAL, move || {
        poll();
        glib::ControlFlow::Continue
    });

    window
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- raw-value extraction ----

    #[test]
    fn celsius_value_extracts_supported() {
        assert_eq!(
            celsius_value(&CapabilityState::Supported(Celsius(55.8))),
            Some(55.8)
        );
    }

    #[test]
    fn celsius_value_extracts_hardware_dependent() {
        assert_eq!(
            celsius_value(&CapabilityState::HardwareDependent(Celsius(42.0))),
            Some(42.0)
        );
    }

    #[test]
    fn celsius_value_none_for_every_other_state() {
        assert_eq!(celsius_value(&CapabilityState::Unsupported), None);
        assert_eq!(celsius_value(&CapabilityState::Unknown), None);
        assert_eq!(celsius_value(&CapabilityState::RequiresPrivilege), None);
    }

    #[test]
    fn percent_value_extracts_supported() {
        assert_eq!(
            percent_value(&CapabilityState::Supported(Percent(6.2))),
            Some(6.2)
        );
    }

    #[test]
    fn megahertz_value_extracts_supported() {
        assert_eq!(
            megahertz_value(&CapabilityState::Supported(Megahertz(1668.0))),
            Some(1668.0)
        );
    }

    #[test]
    fn ram_value_computes_gib_and_percent() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let usage = MemoryUsage {
            total_bytes: 16 * GIB,
            used_bytes: 8 * GIB,
        };
        let (used, total, percent) = ram_value(&CapabilityState::Supported(usage)).unwrap();
        assert!((used - 8.0).abs() < 0.01, "used={used}");
        assert!((total - 16.0).abs() < 0.01, "total={total}");
        assert!((percent - 50.0).abs() < 0.01, "percent={percent}");
    }

    #[test]
    fn ram_value_none_when_unsupported() {
        assert_eq!(ram_value(&CapabilityState::Unsupported), None);
    }

    #[test]
    fn battery_value_formats_not_charging_with_space() {
        let state = CapabilityState::Supported(BatteryState {
            percent: 80.0,
            status: BatteryStatus::NotCharging,
            power_watts: Some(0.0),
        });
        let (percent, status, watts) = battery_value(&state).unwrap();
        assert_eq!(percent, 80.0);
        assert_eq!(status, "Not charging");
        assert_eq!(watts, Some(0.0));
    }

    #[test]
    fn battery_value_none_when_watts_absent() {
        let state = CapabilityState::Supported(BatteryState {
            percent: 100.0,
            status: BatteryStatus::Full,
            power_watts: None,
        });
        let (_, _, watts) = battery_value(&state).unwrap();
        assert_eq!(watts, None);
    }

    #[test]
    fn fan_rpm_values_extracts_supported_vec() {
        let state = CapabilityState::Supported(vec![Rpm(3032), Rpm(2675)]);
        assert_eq!(fan_rpm_values(&state), Some(vec![3032, 2675]));
    }

    #[test]
    fn fan_rpm_values_none_when_unsupported() {
        assert_eq!(fan_rpm_values(&CapabilityState::Unsupported), None);
    }

    // ---- profile display text ----

    #[test]
    fn profile_display_name_replaces_hyphen_and_capitalizes_first_letter_only() {
        assert_eq!(profile_display_name("power-saver"), "Power saver");
        assert_eq!(profile_display_name("balanced"), "Balanced");
        assert_eq!(profile_display_name("performance"), "Performance");
    }

    #[test]
    fn profile_subtitle_known_names() {
        assert_eq!(profile_subtitle("power-saver"), "Quiet, longest runtime");
        assert_eq!(profile_subtitle("balanced"), "Default for daily use");
        assert_eq!(profile_subtitle("performance"), "Full clocks, loud fans");
    }

    #[test]
    fn profile_subtitle_unknown_name_is_empty_not_guessed() {
        assert_eq!(profile_subtitle("turbo-boost-9000"), "");
    }

    // ---- profile_options_detailed ----

    struct FakeProfileProvider {
        details: CapabilityState<Vec<ProfileInfo>>,
        current: CapabilityState<nitroctl_core::power_profile::ProfileStatus>,
    }

    impl PowerProfileProvider for FakeProfileProvider {
        fn list_profiles(&self) -> CapabilityState<Vec<String>> {
            unimplemented!("not used by profile_options_detailed")
        }
        fn list_profile_details(&self) -> CapabilityState<Vec<ProfileInfo>> {
            self.details.clone()
        }
        fn current_profile(&self) -> CapabilityState<nitroctl_core::power_profile::ProfileStatus> {
            self.current.clone()
        }
        fn set_profile(&self, _profile: &str) -> Result<(), ProfileError> {
            unimplemented!("not used by profile_options_detailed")
        }
    }

    fn two_details() -> Vec<ProfileInfo> {
        vec![
            ProfileInfo {
                name: "balanced".to_string(),
                is_placeholder: true,
                known_unsupported: false,
            },
            ProfileInfo {
                name: "performance".to_string(),
                is_placeholder: false,
                known_unsupported: true,
            },
        ]
    }

    #[test]
    fn profile_options_detailed_pairs_full_info_with_the_current_name() {
        let provider = FakeProfileProvider {
            details: CapabilityState::Supported(two_details()),
            current: CapabilityState::Supported(nitroctl_core::power_profile::ProfileStatus {
                name: "balanced".to_string(),
                hardware_backed: false,
            }),
        };

        let result = profile_options_detailed(&provider);

        assert_eq!(result, Some((two_details(), "balanced".to_string())));
    }

    #[test]
    fn profile_options_detailed_none_when_list_unsupported() {
        let provider = FakeProfileProvider {
            details: CapabilityState::Unsupported,
            current: CapabilityState::Unsupported,
        };

        assert_eq!(profile_options_detailed(&provider), None);
    }

    #[test]
    fn profile_error_message_known_unsupported_names_it_a_hardware_limitation() {
        // Issue #25: distinct from a raw BackendFailed errno string.
        let msg = profile_error_message(&ProfileError::KnownUnsupportedProfile {
            requested: "performance".to_string(),
        });

        assert!(msg.contains("performance"), "{msg}");
        assert!(
            !msg.to_lowercase().contains("input/output error"),
            "should not read like a raw errno: {msg}"
        );
    }
}
