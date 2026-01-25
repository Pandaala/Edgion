//! New conf_mgr module with enhanced ResourceProcessor
//!
//! This module provides a redesigned configuration management system where:
//! - ResourceProcessor<T> holds ServerCache<T> and manages the complete resource lifecycle
//! - ProcessorRegistry provides global access to all processors
//! - ProcessorHandler<T> trait defines resource-specific processing logic
//!
//! ## Architecture
//!
//! ```text
//! ConfMgr (facade)
//! └── Arc<dyn ConfCenter>
//!     ├── FileSystemCenter (FileSystem mode)
//!     └── KubernetesCenter (Kubernetes mode)
//!
//! ConfCenter = CenterApi + CenterLifeCycle (super trait)
//! ```
//!
//! ## Key Components
//!
//! - `ConfMgr`: Unified configuration manager facade
//! - `ConfCenter`: Super trait combining CenterApi + CenterLifeCycle
//! - `CenterApi`: CRUD operations trait (replaces old ConfCenter)
//! - `CenterLifeCycle`: Lifecycle management trait
//! - `FileSystemCenter` / `KubernetesCenter`: Concrete implementations
//! - `ProcessorRegistry`: Global registry of all processors
//! - `ResourceProcessor<T>`: Enhanced processor that holds cache, workqueue, and handler
//!
//! ## Usage
//!
//! ```ignore
//! // Create ConfMgr based on configuration
//! let conf_mgr = Arc::new(ConfMgr::create(config).await?);
//!
//! // Start the configuration center
//! conf_mgr.start_with_shutdown(shutdown_handle).await?;
//!
//! // Access CRUD operations (via CenterApi)
//! let content = conf_mgr.get_one("HTTPRoute", Some("default"), "my-route").await?;
//!
//! // Access lifecycle (via CenterLifeCycle)
//! let is_ready = conf_mgr.is_ready();
//! ```

pub mod conf_center;
mod conf_mgr;
mod conf_mgr_trait;
pub mod processor_registry;
mod schema_validator;
pub mod sync_runtime;

// ==================== Top-level exports ====================

// ConfMgr - main entry point
pub use conf_mgr::ConfMgr;

// Configuration (from conf_center)
pub use conf_center::{
    ConfCenterConfig, EndpointMode, FileSystemConfig, KubernetesConfig, LeaderElectionConfig, MetadataFilterConfig,
};

// ProcessorRegistry exports
pub use processor_registry::{ProcessorRegistry, PROCESSOR_REGISTRY};
pub use sync_runtime::resource_processor::{
    HandlerContext, ProcessResult, ProcessorHandler, ProcessorObj, ResourceProcessor,
};

// Re-export commonly used sync_runtime types
pub use sync_runtime::{
    ShutdownController, ShutdownHandle, ShutdownSignal, WorkItem, Workqueue, WorkqueueConfig, WorkqueueMetrics,
};

// Export top-level types
pub use conf_mgr_trait::{ConfMgrError, EdgionConfMgr};
pub use schema_validator::{SchemaValidator, ValidationError};

// ==================== conf_center exports ====================

// Traits
pub use conf_center::traits::{
    CenterApi, CenterLifeCycle, ConfCenter, ConfEntry, ConfWriterError, ListOptions, ListResult,
};

// FileSystem implementations
pub use conf_center::file_system::{FileSystemCenter, FileSystemController, FileSystemStorage};

// Kubernetes implementations
pub use conf_center::kubernetes::{
    ControllerExitReason, KubernetesCenter, KubernetesController, KubernetesStorage, NamespaceWatchMode,
};

// Leader election
pub use conf_center::kubernetes::{LeaderElection, LeaderHandle, RelinkReason};

// Status store
pub use conf_center::status::{FileSystemStatusStore, KubernetesStatusStore, StatusStore, StatusStoreError};

// ==================== Backward compatibility aliases ====================

pub use conf_center::traits::ConfWriterError as ConfStoreError;
