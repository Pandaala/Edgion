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

mod center;
pub mod config;
mod controller;
mod leader_election;
mod namespace;
mod resource_controller;
pub mod status;
mod storage;
mod version_detection;

pub use center::KubernetesCenter;
pub use config::{HaMode, KubernetesConfig, LeaderElectionConfig, MetadataFilterConfig};
pub use controller::{ControllerExitReason, KubernetesController};
pub use leader_election::{LeaderElection, LeaderHandle};
pub use namespace::NamespaceWatchMode;
pub use resource_controller::{RelinkReason, RelinkSignalSender, ResourceController};
pub use status::{create_shared_handler, KubernetesStatusHandler, SharedStatusHandler};
pub use storage::KubernetesStorage;
pub use version_detection::{detect_endpoint_mode, resolve_endpoint_mode};

// Re-export types from sync_runtime
pub use crate::core::controller::conf_mgr::sync_runtime::{
    ShutdownController, ShutdownHandle, ShutdownSignal, WorkItem, Workqueue, WorkqueueConfig, WorkqueueMetrics,
};

// Re-export processor types
pub use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    ProcessorHandler, ResourceProcessor, SecretRefManager,
};

// Re-export registry
pub use crate::core::controller::conf_mgr::PROCESSOR_REGISTRY;

// Re-export metrics from local sync_runtime
pub use crate::core::controller::conf_mgr::sync_runtime::metrics::{
    controller_metrics, ControllerMetrics, InitSyncTimer, ResourceMetrics,
};
