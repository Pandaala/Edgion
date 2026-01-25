//! Sync Runtime Module
//!
//! Provides common synchronization components that can be reused by different backends:
//! - `Workqueue`: Deduplication + retry work queue
//! - `Shutdown`: Graceful shutdown signal handling
//! - `Metrics`: Controller metrics collection
//! - `ResourceProcessor`: Enhanced processor with cache management
//! - `RefGrant`: ReferenceGrant validation and cross-namespace reference management

pub mod metrics;
pub mod ref_grant;
pub mod resource_processor;
pub mod shutdown;
pub mod workqueue;

// Re-export from local modules
pub use metrics::{controller_metrics, ControllerMetrics, InitSyncTimer, ResourceMetrics};
pub use shutdown::{ShutdownController, ShutdownHandle, ShutdownSignal};
pub use workqueue::{WorkItem, Workqueue, WorkqueueConfig, WorkqueueMetrics};

// Re-export from resource_processor
pub use resource_processor::{HandlerContext, ProcessResult, ProcessorHandler, ProcessorObj, ResourceProcessor};
