//! Kubernetes configuration center module
//!
//! Uses kube-runtime Controller pattern for event-driven reconciliation.
//! Each resource type runs as an independent ResourceController with its own
//! complete 1-8 lifecycle (create store, reflector, wait ready, init, reconcile loop).
//!
//! ## Features
//!
//! - **Graceful Shutdown**: Handles SIGTERM/SIGINT signals for clean shutdown
//! - **Leader Election**: Optional leader election for HA deployments using K8s Lease
//! - **Metrics**: Prometheus metrics for reconciliation monitoring

mod context;
mod controller;
mod error;
mod leader_election;
mod metrics;
mod namespace;
mod reconcilers;
mod resource_controller;
mod shutdown;
mod writer;

pub use controller::{KubernetesController, LeaderElectionMode};
pub use context::ControllerContext;
pub use error::ReconcileError;
pub use leader_election::{LeaderElection, LeaderElectionConfig, LeaderHandle};
pub use metrics::{controller_metrics, ControllerMetrics, InitSyncTimer, ResourceMetrics};
pub use namespace::NamespaceWatchMode;
pub use resource_controller::{ResourceController, ResourceControllerBuilder};
pub use shutdown::{ShutdownHandle, ShutdownSignal};
pub use writer::KubernetesWriter;

// Re-export status types from conf_center::status
pub mod status {
    pub use super::super::status::{KubernetesStatusStore, StatusStore, StatusStoreError};
}

pub use status::{KubernetesStatusStore, StatusStore, StatusStoreError};
