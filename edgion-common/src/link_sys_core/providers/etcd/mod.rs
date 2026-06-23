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

pub mod client;
pub mod config_mapping;
pub mod ops;

pub use client::EtcdLinkClient;
pub use ops::{EtcdLockGuard, WatchEvent, WatchEventType};
