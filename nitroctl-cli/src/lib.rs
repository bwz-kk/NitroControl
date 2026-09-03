//! nitroctl-cli library: pure command logic, kept separate from `main.rs` so
//! it's testable against a fake `SensorProvider` without touching real
//! hardware — per docs/architecture.md, the CLI itself never reads `/sys` or
//! runs `nvidia-smi` directly.

pub mod commands;
