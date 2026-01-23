//! Sync Runtime Module
//!
//! Provides common synchronization components that can be reused by different backends:
//! - `Workqueue`: Deduplication + retry work queue
//! - `Shutdown`: Graceful shutdown signal handling
//! - `ResourceProcessor`: Enhanced processor with cache management

pub mod resource_processor;

// Re-export from old conf_mgr (reuse existing implementations)
pub use crate::core::conf_mgr::conf_center::sync_runtime::shutdown::{
    ShutdownController, ShutdownHandle, ShutdownSignal,
};
pub use crate::core::conf_mgr::conf_center::sync_runtime::workqueue::{
    WorkItem, Workqueue, WorkqueueConfig, WorkqueueMetrics,
};

// Re-export from resource_processor
pub use resource_processor::{HandlerContext, ProcessResult, ProcessorHandler, ProcessorObj, ResourceProcessor};
