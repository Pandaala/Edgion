//! File System based configuration center
//!
//! Provides:
//! - FileSystemWriter: reading/writing local YAML files (used by Admin API)
//! - FileWatcher: monitoring file changes and notifying ConfigServer
//! - resource_applier: dispatches resource changes to ConfigServer

mod resource_applier;
mod watcher;
mod writer;

pub use resource_applier::apply_resource_change;
pub use watcher::FileWatcher;
pub use writer::FileSystemWriter;
