//! Redis LinkSys runtime with standalone, Sentinel and Cluster support.
//! redis-rs owns protocol encoding, multiplexing and Cluster routing; Edgion
//! owns canonical dialing, pool recovery, Sentinel discovery and no-replay policy.
//!
//! # Usage
//!
//! ```ignore
//! use crate::link_sys::get_redis_client;
//!
//! if let Some(client) = get_redis_client("default/redis-main") {
//!     let val = client.get("my-key").await?;
//!     client.set("my-key", "my-value", Some(Duration::from_secs(60))).await?;
//! }
//! ```

pub(crate) mod apply;
pub mod client;
pub mod ops;
mod resolver;

// Re-export key types for convenient access
pub(crate) use apply::RedisApply;
pub use client::RedisLinkClient;
pub use ops::{LockOptions, RedisLockGuard};

mod backend;
mod transport;
