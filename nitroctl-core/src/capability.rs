//! The capability model every provider reports through.
//!
//! Per docs/architecture.md: an unsupported or unknown reading is never
//! coerced to a fake value (e.g. `0`) — it is one of these explicit states,
//! which the CLI/GUI render as explicit text.

/// The outcome of asking a provider for one piece of hardware state.
#[derive(Debug, Clone, PartialEq)]
pub enum CapabilityState<T> {
    /// The capability works and this is the current value.
    Supported(T),
    /// This hardware/kernel combination is confirmed not to expose this capability.
    Unsupported,
    /// The interface was expected to work but reading/parsing it failed
    /// (transient error, malformed data, or an unconfirmed code path).
    Unknown,
    /// Reading or writing this capability requires privileges the caller doesn't have.
    RequiresPrivilege,
    /// The underlying interface exists and answers with this value, but the
    /// answer doesn't necessarily reflect real hardware behavior (e.g. a
    /// power-profile "placeholder" backend still names a real active
    /// profile — it just may not change anything on this hardware).
    HardwareDependent(T),
}
