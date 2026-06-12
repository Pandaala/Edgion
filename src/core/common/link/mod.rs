//! Shared link-system core.
//!
//! Hosts the backend-agnostic link-system primitives (credential state machine,
//! TLS material builder, health type, lifecycle trait) plus the etcd provider,
//! which is the only provider needed by the control plane (the controller's
//! etcd-backed configuration center).
//!
//! Data-plane providers (webhook, redis, elasticsearch, local_file) and the
//! runtime store/dispatcher stay in `gateway::link_sys`; they consume this core
//! via re-exports so existing call sites are unaffected.

pub mod credential;
pub mod health;
pub mod lifecycle;
pub mod providers;
pub mod tls;

pub use health::LinkSysHealth;
