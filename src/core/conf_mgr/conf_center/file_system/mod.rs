//! File System based configuration center
//!
//! Provides:
//! - FileSystemWriter: reading/writing local YAML files (used by Admin API)
//! - FileWatcher: monitoring file changes and notifying ConfigServer

mod watcher;
mod writer;

pub use watcher::FileWatcher;
pub use writer::FileSystemWriter;
