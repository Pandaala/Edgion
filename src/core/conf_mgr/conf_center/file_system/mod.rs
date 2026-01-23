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

pub mod sync_controller;
mod writer;

pub use sync_controller::FileSystemSyncController;
pub use writer::FileSystemWriter;