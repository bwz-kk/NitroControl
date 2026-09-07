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
use nitroctl_core::power_draw::{PowerDrawProvider, RaplPowerBackend};
use nitroctl_core::power_profile::{
    AcerPlatformProfileBackend, FailedBackend, PowerProfileProvider, PowerProfilesDaemon,
    ProfileError, ZbusPowerProfilesBackend,
};
use nitroctl_core::sensor::{Celsius, GpuKind, Percent, SensorProvider};
use nitroctl_core::sysfs::RealSysfsReader;

use crate::format::{self, RowContent};

const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// How many samples the CPU-temperature sparkline keeps — 30 samples at the
/// 2s poll interval above is 1 minute of history, enough to see a real trend
/// without the row growing unboundedly.
const SPARKLINE_HISTORY_LEN: usize = 30;
const SPARKLINE_WIDTH: i32 = 80;
const SPARKLINE_HEIGHT: i32 = 24;
/// GNOME's default accent blue (`#3584e4`) — legible on both the light and
/// dark Adwaita row backgrounds without querying the active theme.
const SPARKLINE_LINE_RGB: (f64, f64, f64) = (0.208, 0.518, 0.894);
const SPARKLINE_FILL_ALPHA: f64 = 0.15;

/// A small inline history graph, mocking up Mission Center/Resources-style
/// live sparklines (per docs/roadmap.md's M11 UI-research note) for one
/// row — CPU Temperature, as a concrete first example. Pure GTK4
/// `DrawingArea` + Cairo, no charting dependency: draws a filled line over
/// the row's own recent samples, right-aligned so the most recent reading
/// sits at the right edge.
struct Sparkline {
    widget: DrawingArea,
    history: Rc<RefCell<VecDeque<f64>>>,
}

impl Sparkline {
    fn new() -> Self {
        let history: Rc<RefCell<VecDeque<f64>>> =
            Rc::new(RefCell::new(VecDeque::with_capacity(SPARKLINE_HISTORY_LEN)));
        let widget = DrawingArea::new();
        widget.set_content_width(SPARKLINE_WIDTH);
        widget.set_content_height(SPARKLINE_HEIGHT);
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
            // the most recent sample always sits at the row's right edge.
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
}

const GAUGE_DIAMETER: i32 = 92;
const GAUGE_ARC_WIDTH: f64 = 7.0;
const GAUGE_TRACK_RGBA: (f64, f64, f64, f64) = (0.5, 0.5, 0.5, 0.25);
const GAUGE_LABEL_RGB: (f64, f64, f64) = (0.5, 0.5, 0.5);
/// A 270° sweep with the gap at the bottom — the common "speedometer" gauge
/// convention (Alienware Command Center's own dashboard, one of this
/// milestone's UI references, uses this exact shape).
const GAUGE_START_ANGLE: f64 = 0.75 * std::f64::consts::PI; // 135°
const GAUGE_SWEEP: f64 = 1.5 * std::f64::consts::PI; // 270°

/// A circular gauge (M13, Alienware-Command-Center-style), replacing plain
/// text for a few headline metrics. Same "plain `DrawingArea` + Cairo, no
/// extra dependency" approach as `Sparkline` — Cairo's toy text API is used
/// for the centered number/unit (no shaping/i18n needs here, just ASCII
/// digits and `°C`/`%`), so no new crate (e.g. pangocairo) is pulled in.
struct Gauge {
    widget: DrawingArea,
    value: Rc<Cell<Option<f64>>>,
}

impl Gauge {
    /// `max_value` scales the arc (a reading at or above it draws a full
    /// sweep); `unit` is appended under the number (`"°C"`/`"%"`); `color`
    /// tints both the arc and the number, distinguishing gauge kinds (e.g.
    /// temperature vs. utilization) rather than implying a safety threshold
    /// this project hasn't validated (SAFE-004: no fabricated meaning).
    fn new(max_value: f64, unit: &'static str, color: (f64, f64, f64)) -> Self {
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

            let (r, g, b) = color;
            let value = draw_value.get();
            if let Some(v) = value {
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

            let number_text = match value {
                Some(v) => format!("{v:.0}"),
                None => "--".to_string(),
            };
            cr.set_source_rgba(r, g, b, 1.0);
            cr.select_font_face(
                "sans-serif",
                cairo::FontSlant::Normal,
                cairo::FontWeight::Bold,
            );
            cr.set_font_size(radius * 0.55);
            if let Ok(extents) = cr.text_extents(&number_text) {
                cr.move_to(
                    cx - extents.width() / 2.0 - extents.x_bearing(),
                    cy - extents.height() / 2.0 - extents.y_bearing(),
                );
                let _ = cr.show_text(&number_text);
            }

            if value.is_some() {
                let (lr, lg, lb) = GAUGE_LABEL_RGB;
                cr.set_source_rgba(lr, lg, lb, 1.0);
                cr.select_font_face(
                    "sans-serif",
                    cairo::FontSlant::Normal,
                    cairo::FontWeight::Normal,
                );
                cr.set_font_size(radius * 0.22);
                if let Ok(extents) = cr.text_extents(unit) {
                    cr.move_to(
                        cx - extents.width() / 2.0 - extents.x_bearing(),
                        cy + radius * 0.4,
                    );
                    let _ = cr.show_text(unit);
                }
            }
        });

        Self { widget, value }
    }

    fn set_value(&self, v: Option<f64>) {
        self.value.set(v);
        self.widget.queue_draw();
    }
}

/// One gauge plus its title label, stacked vertically to match the
/// reference dashboards (a big dial with a short caption underneath).
struct GaugeRow {
    container: gtk4::Box,
    gauge: Gauge,
}

impl GaugeRow {
    fn new(title: &str, max_value: f64, unit: &'static str, color: (f64, f64, f64)) -> Self {
        let gauge = Gauge::new(max_value, unit, color);
        let label = gtk4::Label::new(Some(title));
        label.add_css_class("caption-heading");

        let container = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        container.set_halign(gtk4::Align::Center);
        container.append(&gauge.widget);
        container.append(&label);

        Self { container, gauge }
    }
}

/// Extracts the raw numeric value from a `Celsius` capability state for the
/// sparkline, when there's one to plot — `Unsupported`/`Unknown`/
/// `RequiresPrivilege` have no value, and are simply skipped (the sparkline
/// just doesn't grow that tick, rather than plotting a fabricated point).
fn celsius_value(state: &CapabilityState<Celsius>) -> Option<f64> {
    match state {
        CapabilityState::Supported(Celsius(v)) | CapabilityState::HardwareDependent(Celsius(v)) => {
            Some(*v)
        }
        _ => None,
    }
}

/// Warm orange, used for the two temperature gauges — a plain visual
/// distinction from the utilization gauges' cooler blue, not a fabricated
/// safety threshold (SAFE-004: this project doesn't define one).
const GAUGE_THERMAL_RGB: (f64, f64, f64) = (0.902, 0.494, 0.133);
/// GNOME's default accent blue, same as the sparkline — used for the two
/// utilization gauges.
const GAUGE_ACTIVITY_RGB: (f64, f64, f64) = (0.208, 0.518, 0.894);
/// Laptop CPU/GPU temperatures very rarely exceed 100°C before thermal
/// throttling/shutdown — a natural, intuitive gauge ceiling, matching the
/// Alienware Command Center reference's own 0-100 temperature gauges.
const GAUGE_TEMPERATURE_MAX: f64 = 100.0;
const GAUGE_PERCENT_MAX: f64 = 100.0;

/// Extracts the raw numeric value from a `Percent` capability state, same
/// reasoning as `celsius_value` above.
fn percent_value(state: &CapabilityState<Percent>) -> Option<f64> {
    match state {
        CapabilityState::Supported(Percent(v)) | CapabilityState::HardwareDependent(Percent(v)) => {
            Some(*v)
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

/// M10/FR-010: CPU package power draw over the RAPL-compatible powercap
/// interface. Same statefulness reason as `SensorProvider` above (built
/// once, shared via `Arc`) — the energy-delta rate calculation only
/// produces a real value across two calls on the *same* instance, and here
/// that's naturally satisfied by successive poll ticks rather than a
/// CLI-style artificial sleep.
fn build_power_draw_provider() -> Arc<dyn PowerDrawProvider> {
    Arc::new(RaplPowerBackend::new(RealSysfsReader))
}

/// Every value the dashboard displays, read in one go on the worker thread.
struct Snapshot {
    cpu_temperature: RowContent,
    /// Raw value for the CPU-temperature sparkline — `None` when there's
    /// nothing to plot this tick (`Unsupported`/`Unknown`/`RequiresPrivilege`).
    cpu_temperature_value: Option<f64>,
    igpu_temperature: RowContent,
    dgpu_temperature: RowContent,
    /// M13 gauge value — same metric `dgpu_temperature` already carries as
    /// text, just also plotted on the "dGPU Temp" overview gauge.
    dgpu_temperature_value: Option<f64>,
    cpu_utilization: RowContent,
    cpu_utilization_value: Option<f64>,
    igpu_utilization: RowContent,
    dgpu_utilization: RowContent,
    dgpu_utilization_value: Option<f64>,
    cpu_frequency: RowContent,
    ram_usage: RowContent,
    battery: RowContent,
    fan_rpm: RowContent,
    power_profile: RowContent,
    /// M12: `(all profile names, currently active name)` when both the
    /// profile list and the current profile are available — `None` means
    /// there's nothing to populate the interactive selector with (the
    /// `RowContent` text alone covers what to show instead).
    power_profile_options: Option<(Vec<String>, String)>,
    acer_profile: RowContent,
    acer_profile_options: Option<(Vec<String>, String)>,
    battery_limit: RowContent,
    /// M12: `Some(enabled)` when the switch has a real value to show/edit.
    battery_limit_value: Option<bool>,
    battery_calibration: RowContent,
    power_draw: RowContent,
}

/// M12: `(names, current_name)` when a `PowerProfileProvider`'s list and
/// current-profile reads both succeeded — the shape `ProfileRow::apply`
/// needs to populate and select the right entry in its `AdwComboRow`.
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

/// M12: the raw bool a `BatteryLimitProvider`'s `AdwSwitchRow` needs, when
/// there's a real value to show/edit.
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
    power_draw: &dyn PowerDrawProvider,
) -> Snapshot {
    let cpu_temperature_state = sensors.cpu_temperature();
    let dgpu_temperature_state = sensors.gpu_temperature(GpuKind::Discrete);
    let cpu_utilization_state = sensors.cpu_utilization();
    let dgpu_utilization_state = sensors.gpu_utilization(GpuKind::Discrete);
    Snapshot {
        cpu_temperature_value: celsius_value(&cpu_temperature_state),
        cpu_temperature: format::cpu_temperature_row(&cpu_temperature_state),
        igpu_temperature: format::gpu_temperature_row(
            &sensors.gpu_temperature(GpuKind::Integrated),
        ),
        dgpu_temperature_value: celsius_value(&dgpu_temperature_state),
        dgpu_temperature: format::gpu_temperature_row(&dgpu_temperature_state),
        cpu_utilization_value: percent_value(&cpu_utilization_state),
        cpu_utilization: format::cpu_utilization_row(&cpu_utilization_state),
        igpu_utilization: format::gpu_utilization_row(
            &sensors.gpu_utilization(GpuKind::Integrated),
        ),
        dgpu_utilization_value: percent_value(&dgpu_utilization_state),
        dgpu_utilization: format::gpu_utilization_row(&dgpu_utilization_state),
        cpu_frequency: format::cpu_frequency_row(&sensors.cpu_frequency()),
        ram_usage: format::ram_usage_row(&sensors.ram_usage()),
        battery: format::battery_row(&sensors.battery()),
        fan_rpm: format::fan_rpm_row(&sensors.fan_rpm()),
        power_profile: format::profile_status_row(&profile.current_profile()),
        power_profile_options: profile_options(profile),
        acer_profile: format::profile_status_row(&acer_profile.current_profile()),
        acer_profile_options: profile_options(acer_profile),
        battery_limit: format::battery_limit_row(&battery_limit.health_mode()),
        battery_limit_value: battery_limit_value(&battery_limit.health_mode()),
        battery_calibration: format::battery_calibration_row(
            &battery_calibration.calibration_mode(),
        ),
        power_draw: format::power_draw_row(&power_draw.cpu_package_power()),
    }
}

/// One dashboard row: a title label plus a handle to update its subtitle.
struct DashboardRow {
    widget: adw::ActionRow,
}

impl DashboardRow {
    fn new(title: &str) -> Self {
        let widget = adw::ActionRow::builder().title(title).build();
        Self { widget }
    }

    fn update(&self, content: &RowContent) {
        self.widget.set_subtitle(&content.subtitle);
        if content.available {
            self.widget.remove_css_class("dim-label");
        } else {
            self.widget.add_css_class("dim-label");
        }
    }
}

fn profile_error_message(e: &ProfileError) -> String {
    match e {
        ProfileError::InvalidProfile { requested, valid } => format!(
            "Invalid profile {requested:?}; valid choices: {}",
            valid.join(", ")
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

/// An editable "pick one of N named profiles" row (M12) — `power_profile`
/// and `acer_profile` both use this. Keeps the same subtitle-text display
/// as every other row (the same `RowContent` convention `DashboardRow`
/// uses, for every capability state) and layers a real `AdwComboRow`
/// selector on top when a profile list is actually available: picking a
/// different entry calls the provider's `set_profile(name)` off the main
/// thread. A failed write is surfaced via an `AdwToast`; no manual revert
/// is needed since the next poll tick (≤2s later) always re-syncs the
/// selection to the real backend state regardless of whether the write
/// succeeded.
struct ProfileRow {
    widget: adw::ComboRow,
    /// Guards against the poll loop's own `set_model`/`set_selected` calls
    /// (needed to keep the row in sync with real backend state) firing the
    /// same `notify::selected` handler a real user click would — set just
    /// around those calls, same `Cell<bool>` idiom `poll_in_flight` already
    /// uses elsewhere in this file.
    applying_from_poll: Rc<Cell<bool>>,
    /// Only rebuild the `gtk4::StringList` model when the profile set
    /// actually changes, so an unrelated poll tick doesn't reset it.
    last_names: Rc<RefCell<Vec<String>>>,
}

impl ProfileRow {
    fn new(
        title: &str,
        provider: Arc<dyn PowerProfileProvider>,
        toasts: adw::ToastOverlay,
    ) -> Self {
        let widget = adw::ComboRow::builder().title(title).build();
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

    fn apply(&self, content: &RowContent, options: &Option<(Vec<String>, String)>) {
        self.applying_from_poll.set(true);
        match options {
            Some((names, current)) => {
                let mut last_names = self.last_names.borrow_mut();
                if *last_names != *names {
                    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                    self.widget.set_model(Some(&gtk4::StringList::new(&refs)));
                    *last_names = names.clone();
                }
                if let Some(idx) = names.iter().position(|n| n == current) {
                    self.widget.set_selected(idx as u32);
                }
                self.widget.set_sensitive(true);
            }
            None => {
                // Nothing real to select from -- show the same state word
                // the subtitle already carries, as the sole (disabled) item.
                self.widget
                    .set_model(Some(&gtk4::StringList::new(&[content.subtitle.as_str()])));
                self.widget.set_selected(0);
                self.last_names.borrow_mut().clear();
                self.widget.set_sensitive(false);
            }
        }
        self.applying_from_poll.set(false);

        self.widget.set_subtitle(&content.subtitle);
        if content.available {
            self.widget.remove_css_class("dim-label");
        } else {
            self.widget.add_css_class("dim-label");
        }
    }
}

/// An editable on/off row (M12) — `battery_limit`'s `AdwSwitchRow`. Same
/// poll-guard and revert-via-next-poll reasoning as `ProfileRow`.
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

    fn apply(&self, content: &RowContent, value: Option<bool>) {
        self.applying_from_poll.set(true);
        if let Some(enabled) = value {
            self.widget.set_active(enabled);
        }
        self.widget.set_sensitive(value.is_some());
        self.applying_from_poll.set(false);

        self.widget.set_subtitle(&content.subtitle);
        if content.available {
            self.widget.remove_css_class("dim-label");
        } else {
            self.widget.add_css_class("dim-label");
        }
    }
}

struct Dashboard {
    /// M13: the four headline-metric gauges shown above every other group.
    cpu_temperature_gauge: Gauge,
    dgpu_temperature_gauge: Gauge,
    cpu_utilization_gauge: Gauge,
    dgpu_utilization_gauge: Gauge,
    cpu_temperature: DashboardRow,
    cpu_temperature_sparkline: Sparkline,
    igpu_temperature: DashboardRow,
    dgpu_temperature: DashboardRow,
    cpu_utilization: DashboardRow,
    igpu_utilization: DashboardRow,
    dgpu_utilization: DashboardRow,
    cpu_frequency: DashboardRow,
    ram_usage: DashboardRow,
    battery: DashboardRow,
    fan_rpm: DashboardRow,
    power_profile: ProfileRow,
    acer_profile: ProfileRow,
    battery_limit: BatteryLimitRow,
    battery_calibration: DashboardRow,
    power_draw: DashboardRow,
}

impl Dashboard {
    fn apply(&self, snapshot: &Snapshot) {
        self.cpu_temperature_gauge
            .set_value(snapshot.cpu_temperature_value);
        self.dgpu_temperature_gauge
            .set_value(snapshot.dgpu_temperature_value);
        self.cpu_utilization_gauge
            .set_value(snapshot.cpu_utilization_value);
        self.dgpu_utilization_gauge
            .set_value(snapshot.dgpu_utilization_value);

        self.cpu_temperature.update(&snapshot.cpu_temperature);
        if let Some(value) = snapshot.cpu_temperature_value {
            self.cpu_temperature_sparkline.push(value);
        }
        self.igpu_temperature.update(&snapshot.igpu_temperature);
        self.dgpu_temperature.update(&snapshot.dgpu_temperature);
        self.cpu_utilization.update(&snapshot.cpu_utilization);
        self.igpu_utilization.update(&snapshot.igpu_utilization);
        self.dgpu_utilization.update(&snapshot.dgpu_utilization);
        self.cpu_frequency.update(&snapshot.cpu_frequency);
        self.ram_usage.update(&snapshot.ram_usage);
        self.battery.update(&snapshot.battery);
        self.fan_rpm.update(&snapshot.fan_rpm);
        self.power_profile
            .apply(&snapshot.power_profile, &snapshot.power_profile_options);
        self.acer_profile
            .apply(&snapshot.acer_profile, &snapshot.acer_profile_options);
        self.battery_limit
            .apply(&snapshot.battery_limit, snapshot.battery_limit_value);
        self.battery_calibration
            .update(&snapshot.battery_calibration);
        self.power_draw.update(&snapshot.power_draw);
    }
}

fn group(title: &str, rows: &[&adw::ActionRow]) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title(title).build();
    for row in rows {
        group.add(*row);
    }
    group
}

pub fn build_window(app: &adw::Application) -> adw::ApplicationWindow {
    // Shared across every write-failure toast, and across the poll loop's
    // own reads below (M12: same instance either way, no separate build).
    let toast_overlay = adw::ToastOverlay::new();
    let profile_provider = build_profile_provider();
    let acer_profile_provider = build_acer_profile_provider();
    let battery_limit_provider = build_battery_limit_provider();

    // M13: four headline gauges, Alienware-Command-Center-style, above
    // every other group.
    let cpu_temperature_row =
        GaugeRow::new("CPU Temp", GAUGE_TEMPERATURE_MAX, "°C", GAUGE_THERMAL_RGB);
    let dgpu_temperature_row =
        GaugeRow::new("dGPU Temp", GAUGE_TEMPERATURE_MAX, "°C", GAUGE_THERMAL_RGB);
    let cpu_utilization_row = GaugeRow::new("CPU Load", GAUGE_PERCENT_MAX, "%", GAUGE_ACTIVITY_RGB);
    let dgpu_utilization_row =
        GaugeRow::new("dGPU Load", GAUGE_PERCENT_MAX, "%", GAUGE_ACTIVITY_RGB);
    let gauge_grid = gtk4::Grid::builder()
        .row_spacing(8)
        .column_spacing(8)
        .halign(gtk4::Align::Center)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    gauge_grid.attach(&cpu_temperature_row.container, 0, 0, 1, 1);
    gauge_grid.attach(&dgpu_temperature_row.container, 1, 0, 1, 1);
    gauge_grid.attach(&cpu_utilization_row.container, 0, 1, 1, 1);
    gauge_grid.attach(&dgpu_utilization_row.container, 1, 1, 1, 1);

    let cpu_temperature = DashboardRow::new("CPU Temperature");
    let cpu_temperature_sparkline = Sparkline::new();
    cpu_temperature
        .widget
        .add_suffix(&cpu_temperature_sparkline.widget);
    let igpu_temperature = DashboardRow::new("iGPU Temperature");
    let dgpu_temperature = DashboardRow::new("dGPU Temperature");
    let cpu_utilization = DashboardRow::new("CPU Utilization");
    let igpu_utilization = DashboardRow::new("iGPU Utilization");
    let dgpu_utilization = DashboardRow::new("dGPU Utilization");
    let cpu_frequency = DashboardRow::new("CPU Frequency");
    let ram_usage = DashboardRow::new("RAM Usage");
    let battery = DashboardRow::new("Battery");
    let fan_rpm = DashboardRow::new("Fan RPM");
    let power_profile = ProfileRow::new(
        "Power Profile",
        profile_provider.clone(),
        toast_overlay.clone(),
    );
    let acer_profile = ProfileRow::new(
        "Acer Firmware Profile",
        acer_profile_provider.clone(),
        toast_overlay.clone(),
    );
    let battery_limit = BatteryLimitRow::new(
        "Battery Charge Limit",
        battery_limit_provider.clone(),
        toast_overlay.clone(),
    );
    let battery_calibration = DashboardRow::new("Battery Calibration Mode");
    let power_draw = DashboardRow::new("CPU Package Power");

    let cpu_group = group(
        "CPU",
        &[
            &cpu_temperature.widget,
            &cpu_utilization.widget,
            &cpu_frequency.widget,
            &power_draw.widget,
        ],
    );
    let gpu_group = group(
        "GPU",
        &[
            &igpu_temperature.widget,
            &dgpu_temperature.widget,
            &igpu_utilization.widget,
            &dgpu_utilization.widget,
        ],
    );
    let memory_group = group("Memory", &[&ram_usage.widget]);
    let battery_group = group(
        "Battery",
        &[
            &battery.widget,
            battery_limit.widget.upcast_ref::<adw::ActionRow>(),
            &battery_calibration.widget,
        ],
    );
    let fans_group = group("Fans", &[&fan_rpm.widget]);
    let power_group = group(
        "Power Profile",
        &[
            power_profile.widget.upcast_ref::<adw::ActionRow>(),
            acer_profile.widget.upcast_ref::<adw::ActionRow>(),
        ],
    );

    let overview_group = adw::PreferencesGroup::builder().title("Overview").build();
    overview_group.add(&gauge_grid);

    let page = adw::PreferencesPage::new();
    page.add(&overview_group);
    page.add(&cpu_group);
    page.add(&gpu_group);
    page.add(&memory_group);
    page.add(&battery_group);
    page.add(&fans_group);
    page.add(&power_group);

    toast_overlay.set_child(Some(&page));

    let header_bar = adw::HeaderBar::new();
    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header_bar);
    toolbar_view.set_content(Some(&toast_overlay));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("NitroControl")
        // M13: narrower/taller than before (was 480x640) -- a clearly
        // vertical/portrait proportion, matching the NitroSense/PredatorSense
        // reference UIs' own tall-panel layout rather than a wide dashboard.
        .default_width(400)
        .default_height(760)
        .content(&toolbar_view)
        .build();

    let dashboard = Rc::new(Dashboard {
        cpu_temperature_gauge: cpu_temperature_row.gauge,
        dgpu_temperature_gauge: dgpu_temperature_row.gauge,
        cpu_utilization_gauge: cpu_utilization_row.gauge,
        dgpu_utilization_gauge: dgpu_utilization_row.gauge,
        cpu_temperature,
        cpu_temperature_sparkline,
        igpu_temperature,
        dgpu_temperature,
        cpu_utilization,
        igpu_utilization,
        dgpu_utilization,
        cpu_frequency,
        ram_usage,
        battery,
        fan_rpm,
        power_profile,
        acer_profile,
        battery_limit,
        battery_calibration,
        power_draw,
    });

    // Built once, shared across every poll — see the module doc comment
    // for why a fresh provider per tick would break cpu_utilization.
    // (profile_provider/acer_profile_provider/battery_limit_provider were
    // already built above, shared with the M12 editable rows.)
    let sensors = build_sensor_provider();
    let battery_calibration_provider = build_battery_calibration_provider();
    let power_draw_provider = build_power_draw_provider();

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
            let power_draw_provider = power_draw_provider.clone();
            let poll_in_flight = poll_in_flight.clone();
            glib::MainContext::default().spawn_local(async move {
                let result = gio::spawn_blocking(move || {
                    take_snapshot(
                        sensors.as_ref(),
                        profile_provider.as_ref(),
                        acer_profile_provider.as_ref(),
                        battery_limit_provider.as_ref(),
                        battery_calibration_provider.as_ref(),
                        power_draw_provider.as_ref(),
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
