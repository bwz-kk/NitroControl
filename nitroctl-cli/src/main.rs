use clap::{Parser, Subcommand};
use nitroctl_cli::commands::{
    run_battery, run_diagnose, run_fans, run_sensors, run_status, CommandOutput,
};
use nitroctl_core::command::RealCommandRunner;
use nitroctl_core::dmi;
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
    /// Capability matrix + evidence, for GitHub bug reports.
    Diagnose,
}

fn main() {
    let cli = Cli::parse();
    let provider = dmi::build_sensor_provider(RealSysfsReader, RealCommandRunner);

    let CommandOutput { text, exit_code } = match cli.command {
        Command::Status => run_status(provider.as_ref()),
        Command::Sensors => run_sensors(provider.as_ref()),
        Command::Battery => run_battery(provider.as_ref()),
        Command::Fans => run_fans(provider.as_ref()),
        Command::Diagnose => run_diagnose(provider.as_ref()),
    };

    println!("{text}");
    std::process::exit(exit_code);
}
