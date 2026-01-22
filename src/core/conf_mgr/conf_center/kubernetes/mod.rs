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
mod namespace;
mod resource_controller;
mod version_detection;
mod writer;

pub use controller::{ControllerExitReason, KubernetesController};
pub use leader_election::{LeaderElection, LeaderElectionConfig, LeaderHandle};
pub use namespace::NamespaceWatchMode;
pub use resource_controller::{RelinkReason, RelinkSignalSender, ResourceController, ResourceControllerBuilder};
pub use version_detection::{detect_endpoint_mode, resolve_endpoint_mode};
pub use writer::KubernetesWriter;

// Re-export types from sync_runtime for backwards compatibility
pub use super::sync_runtime::{
    controller_metrics, ControllerMetrics, InitSyncTimer, ResourceMetrics,
    ShutdownController, ShutdownHandle, ShutdownSignal,
    WorkItem, Workqueue, WorkqueueConfig, WorkqueueMetrics,
};

// Re-export modules for backwards compatibility
pub mod metrics {
    pub use super::super::sync_runtime::metrics::*;
}

pub mod resource_processor {
    pub use super::super::sync_runtime::resource_processor::*;
}

pub mod shutdown {
    pub use super::super::sync_runtime::shutdown::*;
}

pub mod workqueue {
    pub use super::super::sync_runtime::workqueue::*;
}

// Re-export status types from conf_center::status
pub mod status {
    pub use super::super::status::{KubernetesStatusStore, StatusStore, StatusStoreError};
}

pub use status::{KubernetesStatusStore, StatusStore, StatusStoreError};
