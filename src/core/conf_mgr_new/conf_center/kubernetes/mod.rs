//! Kubernetes configuration center module
//!
//! Uses Go operator-style Workqueue pattern for event-driven reconciliation.
//! Each resource type runs as an independent ResourceController with its own
//! complete lifecycle (create store, reflector, wait ready, init, workqueue reconcile loop).
//!
//! ## Key Differences from Old Architecture
//!
//! - ResourceProcessor<K> is now a stateful struct holding ServerCache
//! - Processors are registered to PROCESSOR_REGISTRY on spawn
//! - ResourceController directly calls processor lifecycle methods
//! - No more ConfigServer dependency - processor manages its own cache
//!
//! ## Features
//!
//! - **Graceful Shutdown**: Handles SIGTERM/SIGINT signals for clean shutdown
//! - **Leader Election**: Optional leader election for HA deployments using K8s Lease
//! - **Metrics**: Prometheus metrics for reconciliation monitoring
//! - **Workqueue**: Go controller-runtime style deduplication and retry with backoff
//! - **ProcessorRegistry**: Global registry for all processors

mod controller;
mod leader_election;
mod namespace;
mod resource_controller;
mod version_detection;

pub use controller::{ControllerExitReason, KubernetesController};
pub use leader_election::{LeaderElection, LeaderElectionConfig, LeaderHandle};
pub use namespace::NamespaceWatchMode;
pub use resource_controller::{RelinkReason, RelinkSignalSender, ResourceController};
pub use version_detection::{detect_endpoint_mode, resolve_endpoint_mode};

// Re-export types from sync_runtime
pub use crate::core::conf_mgr_new::sync_runtime::{
    ShutdownController, ShutdownHandle, ShutdownSignal, WorkItem, Workqueue, WorkqueueConfig, WorkqueueMetrics,
};

// Re-export processor types
pub use crate::core::conf_mgr_new::sync_runtime::resource_processor::{
    ProcessorHandler, ResourceProcessor, SecretRefManager,
};

// Re-export registry
pub use crate::core::conf_mgr_new::PROCESSOR_REGISTRY;

// Re-export metrics from old conf_mgr (shared)
pub use crate::core::conf_mgr::conf_center::sync_runtime::metrics::{
    controller_metrics, ControllerMetrics, InitSyncTimer, ResourceMetrics,
};
