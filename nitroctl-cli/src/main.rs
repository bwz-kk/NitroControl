use clap::{Parser, Subcommand};
use nitroctl_cli::commands::{
    run_battery, run_diagnose, run_fans, run_profile_get, run_profile_list, run_profile_set,
    run_sensors, run_status, CommandOutput,
};
use nitroctl_core::command::RealCommandRunner;
use nitroctl_core::dmi;
use nitroctl_core::power_profile::{FailedBackend, PowerProfilesDaemon, ZbusPowerProfilesBackend};
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
    };

    println!("{text}");
    std::process::exit(exit_code);
}
