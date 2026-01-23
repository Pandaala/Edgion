//! Configuration Center for conf_mgr_new
//!
//! Provides Kubernetes and FileSystem based configuration synchronization.
//!
//! ## Architecture
//!
//! ```text
//! ConfCenter
//! ├── PROCESSOR_REGISTRY (global, holds Arc<dyn ProcessorObj>)
//! │   └── ResourceProcessor<T> for each resource type
//! ├── ConfigSyncServer (for gRPC list/watch)
//! │   └── Uses WatchObj from PROCESSOR_REGISTRY
//! └── ConfWriter (for Admin API CRUD)
//! ```
//!
//! ## Lifecycle
//!
//! - FileSystem mode: `lifecycle_filesystem.rs`
//!   - Runs FileSystemController (registers to PROCESSOR_REGISTRY)
//!   - Creates ConfigSyncServer when ready
//!
//! - Kubernetes mode: `lifecycle_kubernetes.rs`
//!   - Leader election with auto-retry
//!   - Runs KubernetesController (registers to PROCESSOR_REGISTRY)
//!   - Creates ConfigSyncServer when ready

mod conf_center;
mod config;
mod lifecycle_filesystem;
mod lifecycle_kubernetes;

pub mod file_system;
pub mod kubernetes;

// Export configuration types
pub use config::{ConfCenterConfig, EndpointMode, LeaderElectionConfig, MetadataFilterConfig};

// Export ConfCenter
pub use conf_center::ConfCenter;

// Re-export commonly used types from file_system
pub use file_system::{FileSystemController, FileSystemWriter};

// Re-export commonly used types from kubernetes
pub use kubernetes::{
    ControllerExitReason, KubernetesController, LeaderElection, LeaderHandle, NamespaceWatchMode,
};

// Re-export traits from old conf_mgr (for compatibility)
pub use crate::core::conf_mgr::conf_center::traits::{
    ConfEntry, ConfWriter, ConfWriterError, ListOptions, ListResult,
};

// Re-export KubernetesWriter from old conf_mgr (for now)
pub use crate::core::conf_mgr::conf_center::KubernetesWriter;
