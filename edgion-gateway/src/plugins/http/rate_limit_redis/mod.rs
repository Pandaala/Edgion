//! RateLimitRedis plugin module
//!
//! Redis-based precise rate limiting for cluster-wide enforcement.
//! Supports sliding window, fixed window, and token bucket algorithms.

mod plugin;
pub mod scripts;

pub use plugin::RateLimitRedis;
