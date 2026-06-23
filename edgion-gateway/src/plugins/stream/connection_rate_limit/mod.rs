//! L4 connection-rate-limit stream plugin.

pub mod local_gate;
pub mod plugin;

pub use plugin::ConnectionRateLimitPlugin;
