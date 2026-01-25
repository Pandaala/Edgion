//! Configuration Center module
//!
//! Provides configuration center implementations for different backends.
//!
//! ## Architecture
//!
//! ```text
//! ConfCenter = CenterApi + CenterLifeCycle (super trait)
//!     │
//!     ├── FileSystemCenter (implements ConfCenter)
//!     │   ├── CenterApi delegate -> FileSystemWriter
//!     │   └── CenterLifeCycle impl (lifecycle logic)
//!     │
//!     └── KubernetesCenter (implements ConfCenter)
//!         ├── CenterApi delegate -> KubernetesWriter
//!         └── CenterLifeCycle impl (lifecycle logic)
//! ```
//!
//! ## Traits
//!
//! - `CenterApi`: CRUD operations for configuration storage
//! - `CenterLifeCycle`: Lifecycle management (start, reload, shutdown)
//! - `ConfCenter`: Super trait combining CenterApi + CenterLifeCycle
//!
//! ## Configuration
//!
//! - `common`: Common types shared across backends (EndpointMode)
//! - `config`: Top-level ConfCenterConfig enum
//! - `kubernetes::config`: Kubernetes-specific config (LeaderElectionConfig, MetadataFilterConfig)
//!
//! ## Implementations
//!
//! - `FileSystemCenter`: FileSystem-based implementation
//! - `KubernetesCenter`: Kubernetes API-based implementation

pub mod common;
mod config;
pub mod status;
pub mod traits;

pub mod file_system;
pub mod kubernetes;

// Re-export traits
pub use traits::{CenterApi, CenterLifeCycle, ConfCenter, ConfEntry, ConfWriterError, ListOptions, ListResult};

// Re-export FileSystem types
pub use file_system::{FileSystemCenter, FileSystemConfig, FileSystemController, FileSystemWriter};

// Re-export Kubernetes types
pub use kubernetes::{
    ControllerExitReason, KubernetesCenter, KubernetesConfig, KubernetesController, KubernetesWriter, LeaderElection,
    LeaderHandle, LeaderElectionConfig, MetadataFilterConfig, NamespaceWatchMode, RelinkReason,
};

// Re-export status store types
pub use status::{FileSystemStatusStore, KubernetesStatusStore, StatusStore, StatusStoreError};

// Re-export configuration types
pub use common::EndpointMode;
pub use config::ConfCenterConfig;
