//! Etcd LinkSys runtime client.
//!
//! Provides an Etcd client wrapper built from LinkSys CRD config (`EtcdClientConfig`),
//! using [etcd-client](https://crates.io/crates/etcd-client) v0.18 as the underlying library.
//!
//! The patched SDK owns admitted HTTP/2 connection recovery. This module adds:
//! - One background health/discovery monitor with bounded reconnect backoff
//! - Namespace prefix support for key isolation
//! - High-level operations: KV, Lease, Watch, Lock

pub mod client;
pub mod config_mapping;
mod dialer;
mod discovery;
pub mod election;
pub mod observation;
pub mod ops;
mod policy;

pub use client::EtcdLinkClient;
pub use election::{ElectionSession, LeaderKey};
pub use ops::{EtcdLockGuard, KeyValueRaw};
