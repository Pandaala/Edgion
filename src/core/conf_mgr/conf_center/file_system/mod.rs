//! File System based configuration center
//!
//! Provides:
//! - FileSystemWriter: reading/writing local YAML files (used by Admin API)
//! - FileSystemSyncController: unified sync controller (init + runtime)
//! - FileResourceTracker: tracks file-resource mappings for deletion support
//!
//! Deprecated (kept for compatibility):
//! - FileWatcher: legacy file watcher (superseded by FileSystemSyncController)
//! - resource_applier: legacy dispatcher (superseded by ResourceProcessor)

mod resource_applier;
pub mod sync_controller;
pub mod tracker;
mod watcher;
mod writer;

pub use sync_controller::FileSystemSyncController;
pub use tracker::FileResourceTracker;
pub use writer::FileSystemWriter;

// Legacy exports (deprecated, kept for compatibility)
#[deprecated(since = "0.2.0", note = "Use FileSystemSyncController instead")]
pub use watcher::FileWatcher;