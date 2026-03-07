//! Etcd LinkSys runtime client.
//!
//! Provides an Etcd client wrapper built from LinkSys CRD config (`EtcdClientConfig`),
//! using [etcd-client](https://crates.io/crates/etcd-client) v0.18 as the underlying library.
//!
//! Unlike Redis (fred), etcd-client does not have built-in connection pooling or auto-reconnect.
//! This module adds:
//! - Background health monitor with exponential backoff reconnect
//! - Namespace prefix support for key isolation
//! - High-level operations: KV, Lease, Watch, Lock
//!
//! # Usage
//!
//! ```ignore
//! use crate::core::gateway::link_sys::get_etcd_client;
//!
//! if let Some(client) = get_etcd_client("default/etcd-main") {
//!     client.put_string("my-key", "my-value").await?;
//!     let val = client.get_string("my-key").await?;
//! }
//! ```

pub mod client;
pub mod config_mapping;
pub mod ops;

// Re-export key types for convenient access
pub use client::EtcdLinkClient;
pub use ops::{EtcdLockGuard, WatchEvent, WatchEventType};
