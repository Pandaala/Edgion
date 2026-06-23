//! Fault Injection plugin.
//!
//! Probabilistic delay and/or abort at the request_filter stage. Request
//! matching is delegated to the plugin entry's `conditions:` field.

mod plugin;

pub use plugin::FaultInjection;
