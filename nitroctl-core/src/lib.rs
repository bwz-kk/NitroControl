//! nitroctl-core: hardware abstraction and capability model for NitroControl.
//!
//! See docs/architecture.md in the project root for the design this crate implements.

pub mod battery_limit;
pub mod capability;
pub mod command;
pub mod dmi;
pub mod power_profile;
pub mod provider;
pub mod sensor;
pub mod sysfs;
