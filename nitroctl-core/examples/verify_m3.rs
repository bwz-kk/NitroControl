//! Real-hardware verification for M3, per docs/roadmap.md: exercises
//! list/get/set against the real running `power-profiles-daemon`, then
//! restores the original active profile. Not a test — a manual
//! verification aid. Run with:
//!   cargo run -p nitroctl-core --example verify_m3

use nitroctl_core::power_profile::{
    PowerProfileProvider, PowerProfilesDaemon, ZbusPowerProfilesBackend,
};

fn main() {
    let backend = ZbusPowerProfilesBackend::connect().expect("power-profiles-daemon not reachable");
    let provider = PowerProfilesDaemon::new(backend);

    println!("list_profiles:   {:?}", provider.list_profiles());
    let before = provider.current_profile();
    println!("current_profile (before): {before:?}");

    let original_name = match &before {
        nitroctl_core::capability::CapabilityState::Supported(status)
        | nitroctl_core::capability::CapabilityState::HardwareDependent(status) => {
            status.name.clone()
        }
        other => panic!("expected a profile name to restore later, got {other:?}"),
    };

    // Pick a different valid profile to switch to, to prove `set` really
    // changes PPD's own state and not just our local view of it.
    let target = match original_name.as_str() {
        "performance" => "balanced",
        _ => "performance",
    };

    println!(
        "set_profile({target:?}): {:?}",
        provider.set_profile(target)
    );
    println!(
        "current_profile (after set): {:?}",
        provider.current_profile()
    );

    println!("restoring original profile {original_name:?}...");
    provider
        .set_profile(&original_name)
        .expect("failed to restore original profile");
    println!(
        "current_profile (restored): {:?}",
        provider.current_profile()
    );

    println!("\ninvalid profile name is rejected without touching the backend:");
    println!("{:?}", provider.set_profile("turbo-nitro-mode"));
}
