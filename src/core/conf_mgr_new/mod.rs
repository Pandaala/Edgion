//! New conf_mgr module with enhanced ResourceProcessor
//!
//! This module provides a redesigned configuration management system where:
//! - ResourceProcessor<T> holds ServerCache<T> and manages the complete resource lifecycle
//! - ProcessorRegistry provides global access to all processors
//! - ProcessorHandler<T> trait defines resource-specific processing logic
//!
//! ## Key Components
//!
//! - `ProcessorRegistry`: Global registry of all processors, provides typed and dynamic access
//! - `ResourceProcessor<T>`: Enhanced processor that holds cache, workqueue, and handler
//! - `ProcessorHandler<T>`: Trait for resource-specific processing logic
//! - `HandlerContext`: Context for handler methods
//!
//! ## Usage
//!
//! ```ignore
//! // Create a processor
//! let processor = ResourceProcessor::new(
//!     "HTTPRoute",
//!     1000,
//!     Arc::new(HttpRouteHandler),
//!     secret_ref_manager,
//! );
//!
//! // Register to global registry
//! PROCESSOR_REGISTRY.register(processor.clone());
//!
//! // Register WatchObj to ConfigSyncServer
//! config_sync_server.register_watch_obj("HTTPRoute", processor.as_watch_obj());
//! ```

pub mod conf_center;
mod conf_mgr_trait;
pub mod processor_registry;
mod schema_validator;
pub mod sync_runtime;

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

// ConfCenter exports (compatible with old conf_mgr)
pub use conf_center::{
    ConfCenter, ConfCenterConfig, ConfEntry, ConfWriter, ConfWriterError, ControllerExitReason, EndpointMode,
    FileSystemController, FileSystemStatusStore, FileSystemWriter, KubernetesController, KubernetesStatusStore,
    KubernetesWriter, LeaderElectionConfig, MetadataFilterConfig, NamespaceWatchMode, StatusStore, StatusStoreError,
};

// Kubernetes-specific exports
pub use conf_center::kubernetes::{LeaderElection, LeaderHandle, RelinkReason};

// Backward compatibility aliases
pub use conf_center::ConfWriterError as ConfStoreError;
