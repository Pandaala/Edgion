//! Kubernetes configuration center module
//!
//! Uses Go operator-style Workqueue pattern for event-driven reconciliation.
//! Each resource type runs as an independent ResourceController with its own
//! complete 1-8 lifecycle (create store, reflector, wait ready, init, workqueue reconcile loop).
//!
//! ## Features
//!
//! - **Graceful Shutdown**: Handles SIGTERM/SIGINT signals for clean shutdown
//! - **Leader Election**: Optional leader election for HA deployments using K8s Lease
//! - **Metrics**: Prometheus metrics for reconciliation monitoring
//! - **Workqueue**: Go controller-runtime style deduplication and retry with backoff

mod controller;
mod leader_election;
mod metrics;
mod namespace;
mod resource_controller;
pub mod resource_processor;
pub mod shutdown;
mod version_detection;
mod workqueue;
mod writer;

pub use controller::{ControllerExitReason, KubernetesController};
pub use leader_election::{LeaderElection, LeaderElectionConfig, LeaderHandle};
pub use metrics::{controller_metrics, ControllerMetrics, InitSyncTimer, ResourceMetrics};
pub use namespace::NamespaceWatchMode;
pub use resource_controller::{RelinkReason, RelinkSignalSender, ResourceController, ResourceControllerBuilder};
pub use shutdown::{ShutdownHandle, ShutdownSignal};
pub use version_detection::{detect_endpoint_mode, resolve_endpoint_mode};
pub use writer::KubernetesWriter;

// Re-export status types from conf_center::status
pub mod status {
    pub use super::super::status::{KubernetesStatusStore, StatusStore, StatusStoreError};
}

pub use status::{KubernetesStatusStore, StatusStore, StatusStoreError};
