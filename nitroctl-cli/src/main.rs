use clap::{Parser, Subcommand};
use nitroctl_cli::commands::{
    run_acer_profile_get, run_acer_profile_list, run_acer_profile_set, run_battery,
    run_battery_calibrate_get, run_battery_calibrate_set, run_battery_limit_get,
    run_battery_limit_set, run_diagnose, run_fans, run_profile_get, run_profile_list,
    run_profile_set, run_sensors, run_status, CommandOutput,
};
use nitroctl_core::battery_calibration::BatteryCalibrationProvider;
use nitroctl_core::battery_limit::AcerWmiBatteryBackend;
use nitroctl_core::command::RealCommandRunner;
use nitroctl_core::dmi;
use nitroctl_core::power_profile::{
    AcerPlatformProfileBackend, FailedBackend, PowerProfilesDaemon, ZbusPowerProfilesBackend,
};
use nitroctl_core::sysfs::RealSysfsReader;

#[derive(Parser)]
#[command(
    name = "nitroctl",
    about = "Monitoring and control for Acer Nitro laptops"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// One-screen summary: CPU/GPU temp+util, RAM, battery.
    Status,
    /// CPU temp, iGPU temp, dGPU temp, CPU freq, CPU util, RAM usage.
    Sensors,
    /// Battery percentage, status, power draw.
    Battery,
    /// Fan RPM ("unavailable" on unsupported hardware — explicit, not omitted).
    Fans,
    /// Power profile control via power-profiles-daemon.
    #[command(subcommand)]
    Profile(ProfileCommand),
    /// Acer-firmware power profile control via /sys/firmware/acpi/platform_profile
    /// (M5, FR-007) — only real when acer_wmi is loaded with predator_v4=1,
    /// separate from `profile` (see docs/architecture.md's M5 design section).
    #[command(subcommand)]
    AcerProfile(ProfileCommand),
    /// Battery charge limit (M6, FR-008) via the out-of-tree
    /// bwz-kk/acer-wmi-battery driver's health_mode -- only real when that
    /// module is built and loaded (see docs/optional-setup.md).
    #[command(subcommand)]
    BatteryLimit(BatteryLimitCommand),
    /// Battery calibration mode (M7, FR-009) via the same out-of-tree
    /// driver's calibration_mode -- a multi-hour discharge/recharge cycle,
    /// not a persistent setting (see docs/architecture.md's M7 design
    /// section). `set on` starts it; the driver never signals completion.
    #[command(subcommand)]
    BatteryCalibrate(BatteryCalibrateCommand),
    /// Capability matrix + evidence, for GitHub bug reports.
    Diagnose,
}

#[derive(Subcommand)]
enum ProfileCommand {
    /// List available power profiles.
    List,
    /// Show the currently active power profile.
    Get,
    /// Set the active power profile.
    Set { name: String },
}

#[derive(Subcommand)]
enum BatteryLimitCommand {
    /// Show whether the battery charge limit (health mode) is on.
    Get,
    /// Turn the battery charge limit on or off ("on" or "off").
    Set { state: String },
}

#[derive(Subcommand)]
enum BatteryCalibrateCommand {
    /// Show whether calibration mode is currently running.
    Get,
    /// Start or stop a calibration cycle ("on" or "off"). Starting one
    /// disables the charge limit and begins a multi-hour discharge/recharge
    /// cycle the driver won't tell you the end of -- see the `set on`
    /// output for the full caution.
    Set { state: String },
}

fn main() {
    let cli = Cli::parse();
    let sensor_provider = || dmi::build_sensor_provider(RealSysfsReader, RealCommandRunner);

    let CommandOutput { text, exit_code } = match cli.command {
        Command::Status => run_status(sensor_provider().as_ref()),
        Command::Sensors => run_sensors(sensor_provider().as_ref()),
        Command::Battery => run_battery(sensor_provider().as_ref()),
        Command::Fans => run_fans(sensor_provider().as_ref()),
        Command::Diagnose => run_diagnose(sensor_provider().as_ref()),
        Command::Profile(profile_command) => {
            // Connecting never hard-fails the CLI: on failure, FailedBackend
            // carries the *actual* connect() error through to the usual
            // capability-state mapping (Unavailable -> Unsupported, Denied ->
            // RequiresPrivilege, Other -> Unknown) instead of collapsing
            // every failure into "unavailable" — a real AccessDenied must
            // not be misreported as "service not installed".
            let profile_provider: Box<dyn nitroctl_core::power_profile::PowerProfileProvider> =
                match ZbusPowerProfilesBackend::connect() {
                    Ok(backend) => Box::new(PowerProfilesDaemon::new(backend)),
                    Err(e) => Box::new(PowerProfilesDaemon::new(FailedBackend(e))),
                };
            match profile_command {
                ProfileCommand::List => run_profile_list(profile_provider.as_ref()),
                ProfileCommand::Get => run_profile_get(profile_provider.as_ref()),
                ProfileCommand::Set { name } => run_profile_set(profile_provider.as_ref(), &name),
            }
        }
        Command::AcerProfile(profile_command) => {
            // No connect() step, and so nothing that can fail up front like
            // ZbusPowerProfilesBackend::connect() above — this backend is
            // just sysfs reads/writes, so "not available" only shows up
            // per-call (Unavailable, when the sysfs nodes are absent — the
            // default state, since NitroControl never loads predator_v4=1
            // itself).
            let acer_profile_provider =
                PowerProfilesDaemon::new(AcerPlatformProfileBackend::new(RealSysfsReader));
            match profile_command {
                ProfileCommand::List => run_acer_profile_list(&acer_profile_provider),
                ProfileCommand::Get => run_acer_profile_get(&acer_profile_provider),
                ProfileCommand::Set { name } => run_acer_profile_set(&acer_profile_provider, &name),
            }
        }
        Command::BatteryLimit(battery_limit_command) => {
            // Same shape as AcerProfile above: no connect() step, pure
            // sysfs reads/writes, "not available" shows up per-call when
            // the out-of-tree driver isn't loaded (the default).
            let battery_limit_provider = AcerWmiBatteryBackend::new(RealSysfsReader);
            match battery_limit_command {
                BatteryLimitCommand::Get => run_battery_limit_get(&battery_limit_provider),
                BatteryLimitCommand::Set { state } => match state.as_str() {
                    "on" => run_battery_limit_set(&battery_limit_provider, true),
                    "off" => run_battery_limit_set(&battery_limit_provider, false),
                    other => CommandOutput {
                        text: format!("Invalid state {other:?}; valid choices: on, off"),
                        exit_code: 2,
                    },
                },
            }
        }
        Command::BatteryCalibrate(battery_calibrate_command) => {
            // Same AcerWmiBatteryBackend type as BatteryLimit above -- one
            // physical driver instance implements both traits (see
            // battery_calibration's doc comment) -- just a fresh instance
            // per invocation like the rest of this CLI's providers.
            let battery_calibration_provider: Box<dyn BatteryCalibrationProvider> =
                Box::new(AcerWmiBatteryBackend::new(RealSysfsReader));
            match battery_calibrate_command {
                BatteryCalibrateCommand::Get => {
                    run_battery_calibrate_get(battery_calibration_provider.as_ref())
                }
                BatteryCalibrateCommand::Set { state } => match state.as_str() {
                    "on" => run_battery_calibrate_set(battery_calibration_provider.as_ref(), true),
                    "off" => {
                        run_battery_calibrate_set(battery_calibration_provider.as_ref(), false)
                    }
                    other => CommandOutput {
                        text: format!("Invalid state {other:?}; valid choices: on, off"),
                        exit_code: 2,
                    },
                },
            }
        }
    };

    println!("{text}");
    std::process::exit(exit_code);
}
