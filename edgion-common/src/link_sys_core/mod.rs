//! LinkSys core (shared, backend-agnostic).
//!
//! This is the `core` half of the LinkSys subsystem. It hosts the
//! backend-agnostic primitives (credential state machine, TLS material builder,
//! health type) plus the etcd provider, which is the only
//! provider needed by the control plane (the controller's etcd-backed
//! configuration center).
//!
//! The `runtime` half — data-plane providers (webhook, redis, elasticsearch,
//! local_file) and the runtime store/dispatcher — lives in `gateway::link_sys`;
//! it consumes this core via re-exports so existing call sites are unaffected.
//!
//! Placement rule for new code: backend-agnostic core logic (credential / TLS /
//! health, or a provider the controller also needs) belongs here;
//! data-plane runtime providers and stores belong in `gateway::link_sys`.

pub mod credential;
pub mod fingerprint;
pub mod health;
pub mod http_client;
pub mod outbound_tls;
pub mod providers;
pub mod tls;

pub use health::{HealthTransitionReporter, LinkSysHealth, PublishedHealth};
