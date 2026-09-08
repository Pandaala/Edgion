//! Kubernetes configuration center module
//!
//! Uses Go operator-style Workqueue pattern for event-driven reconciliation.
//! Each resource type runs as an independent ResourceController with its own
//! complete lifecycle (create store, reflector, wait ready, init, workqueue reconcile loop).
//!
//! ## Architecture
//!
//! ```text
//! KubernetesCenter (implements ConfCenter = CenterApi + CenterLifeCycle)
//!     │
//!     ├── writer: KubernetesStorage (CenterApi delegate)
//!     │
//!     └── lifecycle (CenterLifeCycle impl)
//!             │
//!             ├── Leader Election
//!             │
//!             └── KubernetesController
//!                     │
//!                     ├── spawn::<HTTPRoute, _>(HttpRouteHandler)
//!                     │       └── ResourceProcessor + ResourceController
//!                     │
//!                     └── spawn::<Gateway, _>(GatewayHandler)  
//!                             └── ResourceProcessor + ResourceController
//! ```
//!
//! ## Features
//!
//! - **Graceful Shutdown**: Handles SIGTERM/SIGINT signals for clean shutdown
//! - **Leader Election**: Optional leader election for HA deployments using K8s Lease
//! - **Metrics**: Prometheus metrics for reconciliation monitoring
//! - **Workqueue**: Go controller-runtime style deduplication with delayed/coalesced requeue
//!   (self-heals on watcher relist; no exponential backoff retry)
//! - **ProcessorRegistry**: Global registry for all processors

mod center;
pub mod config;
mod controller;
mod leader_election;
mod namespace;
mod resource_controller;
mod storage;
mod version_detection;
mod watch_source;

pub use center::KubernetesCenter;
pub use config::{HaMode, KubernetesConfig, LeaderElectionConfig, MetadataFilterConfig};
pub use controller::{ControllerExitReason, KubernetesController};
pub use leader_election::{LeaderElection, LeaderFence, LeaderHandle};
pub use namespace::NamespaceWatchMode;
pub use resource_controller::ResourceController;
pub use storage::KubernetesStorage;
pub use version_detection::{
    detect_reference_grant_version, detect_tls_route_version, probe_tls_route_version, ReferenceGrantVersion,
    TLSRouteVersion,
};

// Re-export types from sync_runtime
pub use crate::conf_mgr::sync_runtime::{
    ShutdownController, ShutdownHandle, ShutdownSignal, WorkItem, Workqueue, WorkqueueConfig, WorkqueueMetrics,
};

// Re-export processor types
pub use crate::conf_mgr::sync_runtime::resource_processor::{ProcessorHandler, ResourceProcessor, SecretRefManager};

// Re-export metrics from local sync_runtime
pub use crate::conf_mgr::sync_runtime::metrics::{
    controller_metrics, ControllerMetrics, InitSyncTimer, ResourceMetrics,
};
