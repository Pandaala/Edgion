//! File System based configuration center
//!
//! Provides:
//! - FileSystemWriter: reading/writing local YAML files (used by Admin API)
//! - FileSystemSyncController: unified sync controller (init + runtime)
//!
//! File naming convention:
//! - With namespace: `{Kind}_{namespace}_{name}.yaml`
//! - Cluster-scoped: `{Kind}__{name}.yaml` (double underscore)
//!
//! This naming convention enables:
//! - Resource identity parsing from filename (no tracking state needed)
//! - Unique filenames (no collision between different resources)
//!
//! Deprecated (kept for compatibility):
//! - FileWatcher: legacy file watcher (superseded by FileSystemSyncController)
//! - resource_applier: legacy dispatcher (superseded by ResourceProcessor)

mod resource_applier;
pub mod sync_controller;
mod watcher;
mod writer;

pub use sync_controller::FileSystemSyncController;
pub use writer::FileSystemWriter;

// Legacy exports (deprecated, kept for compatibility)
#[deprecated(since = "0.2.0", note = "Use FileSystemSyncController instead")]
pub use watcher::FileWatcher;