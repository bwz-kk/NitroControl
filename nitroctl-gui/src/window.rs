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

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk4::{gio, glib};
use libadwaita as adw;

use nitroctl_core::command::RealCommandRunner;
use nitroctl_core::dmi;
use nitroctl_core::power_profile::{
    FailedBackend, PowerProfileProvider, PowerProfilesDaemon, ZbusPowerProfilesBackend,
};
use nitroctl_core::sensor::{GpuKind, SensorProvider};
use nitroctl_core::sysfs::RealSysfsReader;

use crate::format::{self, RowContent};

const POLL_INTERVAL: Duration = Duration::from_secs(2);

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

/// Every value the dashboard displays, read in one go on the worker thread.
struct Snapshot {
    cpu_temperature: RowContent,
    igpu_temperature: RowContent,
    dgpu_temperature: RowContent,
    cpu_utilization: RowContent,
    igpu_utilization: RowContent,
    dgpu_utilization: RowContent,
    cpu_frequency: RowContent,
    ram_usage: RowContent,
    battery: RowContent,
    fan_rpm: RowContent,
    power_profile: RowContent,
}

/// Blocking: reads every sensor + the power profile off the shared,
/// long-lived providers. Must only run on a worker thread
/// (`gio::spawn_blocking`), never the GTK main thread.
fn take_snapshot(sensors: &dyn SensorProvider, profile: &dyn PowerProfileProvider) -> Snapshot {
    Snapshot {
        cpu_temperature: format::cpu_temperature_row(&sensors.cpu_temperature()),
        igpu_temperature: format::gpu_temperature_row(
            &sensors.gpu_temperature(GpuKind::Integrated),
        ),
        dgpu_temperature: format::gpu_temperature_row(&sensors.gpu_temperature(GpuKind::Discrete)),
        cpu_utilization: format::cpu_utilization_row(&sensors.cpu_utilization()),
        igpu_utilization: format::gpu_utilization_row(
            &sensors.gpu_utilization(GpuKind::Integrated),
        ),
        dgpu_utilization: format::gpu_utilization_row(&sensors.gpu_utilization(GpuKind::Discrete)),
        cpu_frequency: format::cpu_frequency_row(&sensors.cpu_frequency()),
        ram_usage: format::ram_usage_row(&sensors.ram_usage()),
        battery: format::battery_row(&sensors.battery()),
        fan_rpm: format::fan_rpm_row(&sensors.fan_rpm()),
        power_profile: format::profile_status_row(&profile.current_profile()),
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

struct Dashboard {
    cpu_temperature: DashboardRow,
    igpu_temperature: DashboardRow,
    dgpu_temperature: DashboardRow,
    cpu_utilization: DashboardRow,
    igpu_utilization: DashboardRow,
    dgpu_utilization: DashboardRow,
    cpu_frequency: DashboardRow,
    ram_usage: DashboardRow,
    battery: DashboardRow,
    fan_rpm: DashboardRow,
    power_profile: DashboardRow,
}

impl Dashboard {
    fn apply(&self, snapshot: &Snapshot) {
        self.cpu_temperature.update(&snapshot.cpu_temperature);
        self.igpu_temperature.update(&snapshot.igpu_temperature);
        self.dgpu_temperature.update(&snapshot.dgpu_temperature);
        self.cpu_utilization.update(&snapshot.cpu_utilization);
        self.igpu_utilization.update(&snapshot.igpu_utilization);
        self.dgpu_utilization.update(&snapshot.dgpu_utilization);
        self.cpu_frequency.update(&snapshot.cpu_frequency);
        self.ram_usage.update(&snapshot.ram_usage);
        self.battery.update(&snapshot.battery);
        self.fan_rpm.update(&snapshot.fan_rpm);
        self.power_profile.update(&snapshot.power_profile);
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
    let cpu_temperature = DashboardRow::new("CPU Temperature");
    let igpu_temperature = DashboardRow::new("iGPU Temperature");
    let dgpu_temperature = DashboardRow::new("dGPU Temperature");
    let cpu_utilization = DashboardRow::new("CPU Utilization");
    let igpu_utilization = DashboardRow::new("iGPU Utilization");
    let dgpu_utilization = DashboardRow::new("dGPU Utilization");
    let cpu_frequency = DashboardRow::new("CPU Frequency");
    let ram_usage = DashboardRow::new("RAM Usage");
    let battery = DashboardRow::new("Battery");
    let fan_rpm = DashboardRow::new("Fan RPM");
    let power_profile = DashboardRow::new("Power Profile");

    let cpu_group = group(
        "CPU",
        &[
            &cpu_temperature.widget,
            &cpu_utilization.widget,
            &cpu_frequency.widget,
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
    let battery_group = group("Battery", &[&battery.widget]);
    let fans_group = group("Fans", &[&fan_rpm.widget]);
    let power_group = group("Power Profile", &[&power_profile.widget]);

    let page = adw::PreferencesPage::new();
    page.add(&cpu_group);
    page.add(&gpu_group);
    page.add(&memory_group);
    page.add(&battery_group);
    page.add(&fans_group);
    page.add(&power_group);

    let header_bar = adw::HeaderBar::new();
    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header_bar);
    toolbar_view.set_content(Some(&page));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("NitroControl")
        .default_width(480)
        .default_height(640)
        .content(&toolbar_view)
        .build();

    let dashboard = Rc::new(Dashboard {
        cpu_temperature,
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
    });

    // Built once, shared across every poll — see the module doc comment
    // for why a fresh provider per tick would break cpu_utilization.
    let sensors = build_sensor_provider();
    let profile_provider = build_profile_provider();

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
            let poll_in_flight = poll_in_flight.clone();
            glib::MainContext::default().spawn_local(async move {
                let result = gio::spawn_blocking(move || {
                    take_snapshot(sensors.as_ref(), profile_provider.as_ref())
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
