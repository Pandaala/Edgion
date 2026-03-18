//! Sync Runtime Module
//!
//! Provides common synchronization components that can be reused by different backends:
//! - `Workqueue`: Deduplication + retry work queue
//! - `Shutdown`: Graceful shutdown signal handling
//! - `Metrics`: Controller metrics collection
//! - `ResourceProcessor`: Enhanced processor with cache management

pub mod metrics;
pub mod resource_processor;
pub mod shutdown;
pub mod workqueue;

// Re-export from local modules
pub use metrics::{
    controller_metrics, record_status_write, record_status_write_error, record_status_write_skipped, ControllerMetrics,
    InitSyncTimer, ResourceMetrics,
};
pub use shutdown::{ShutdownController, ShutdownHandle, ShutdownSignal};
pub use workqueue::{WorkItem, Workqueue, WorkqueueConfig, WorkqueueMetrics};

// Re-export from resource_processor
pub use resource_processor::{HandlerContext, ProcessResult, ProcessorHandler, ProcessorObj, ResourceProcessor};
