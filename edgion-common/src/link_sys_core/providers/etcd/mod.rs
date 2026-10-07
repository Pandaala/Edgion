//! Etcd LinkSys runtime client.
//!
//! Provides an Etcd client wrapper built from LinkSys CRD config (`EtcdClientConfig`),
//! using [etcd-client](https://crates.io/crates/etcd-client) v0.18 as the underlying library.
//!
//! The upstream SDK owns multiplexed gRPC connections and transport recovery.
//! This module adds:
//! - Background health monitor with exponential backoff reconnect
//! - Namespace prefix support for key isolation
//! - High-level operations: KV, Lease, Watch, Lock

pub mod client;
pub mod config_mapping;
pub mod election;
pub mod ops;

pub use client::{EtcdBootstrapConfig, EtcdLinkClient};
pub use election::{ElectionSession, LeaderKey};
pub use ops::{EtcdLockGuard, KeyValueRaw};
