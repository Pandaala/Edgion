//! Redis LinkSys runtime client.
//!
//! Provides a Redis client wrapper built from LinkSys CRD config (`RedisClientConfig`),
//! using [fred](https://crates.io/crates/fred) v10 as the underlying library.
//!
//! Supports standalone, sentinel, and cluster deployments through fred's unified API.
//! Connection pooling, automatic reconnection, and TLS are all handled by fred internally.
//!
//! # Usage
//!
//! ```ignore
//! use crate::core::gateway::link_sys::providers::redis::get_redis_client;
//!
//! if let Some(client) = get_redis_client("default/redis-main") {
//!     let val = client.get("my-key").await?;
//!     client.set("my-key", "my-value", Some(Duration::from_secs(60))).await?;
//! }
//! ```

pub mod client;
pub mod config_mapping;
pub mod data_sender;
pub mod ops;

// Re-export key types for convenient access
pub use client::RedisLinkClient;
pub use data_sender::RedisDataSender;
pub use ops::{LinkSysHealth, LockOptions, RedisLockGuard};
