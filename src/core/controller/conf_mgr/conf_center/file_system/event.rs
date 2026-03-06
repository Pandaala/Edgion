//! Event types for FileSystem synchronization
//!
//! These events mirror the K8s watcher::Event to enable code reuse
//! between Kubernetes and FileSystem modes.

use std::fmt::Debug;
use std::path::PathBuf;

/// FileSystem event type (mirrors K8s watcher::Event)
///
/// This allows the same event handling logic to be used for both
/// Kubernetes and FileSystem modes.
///
/// Events include file path information to enable status file management.
#[derive(Debug, Clone)]
pub enum FileSystemEvent<K> {
    /// Initialization started (directory scan beginning)
    Init,
    /// Resource found during initialization (file scanned)
    /// Contains the file path and content
    InitApply {
        /// Path to the configuration file
        path: PathBuf,
        /// File content (typically YAML string)
        content: K,
    },
    /// Initialization completed (directory scan done)
    InitDone,
    /// Resource created or modified (file created/changed)
    /// Contains the file path and content
    Apply {
        /// Path to the configuration file
        path: PathBuf,
        /// File content (typically YAML string)
        content: K,
    },
    /// Resource deleted (file removed)
    Delete(K),
}

/// Generic resource event that abstracts over K8s and FileSystem events
///
/// This type is used internally to provide a unified interface for
/// event processing regardless of the source (K8s API or local files).
#[derive(Debug, Clone)]
pub enum ResourceEvent<K> {
    /// Initialization started
    Init,
    /// Resource found during initialization
    InitApply(K),
    /// Initialization completed
    InitDone,
    /// Resource created or modified
    Apply(K),
    /// Resource deleted
    Delete(K),
}

impl<K> From<FileSystemEvent<K>> for ResourceEvent<K> {
    fn from(event: FileSystemEvent<K>) -> Self {
        match event {
            FileSystemEvent::Init => ResourceEvent::Init,
            FileSystemEvent::InitApply { content, .. } => ResourceEvent::InitApply(content),
            FileSystemEvent::InitDone => ResourceEvent::InitDone,
            FileSystemEvent::Apply { content, .. } => ResourceEvent::Apply(content),
            FileSystemEvent::Delete(obj) => ResourceEvent::Delete(obj),
        }
    }
}

/// Parsed file information containing resource identity
#[derive(Debug, Clone)]
pub struct ParsedFileInfo {
    /// Resource kind (e.g., "HTTPRoute", "Gateway")
    pub kind: String,
    /// Resource key ("namespace/name" or just "name")
    pub key: String,
}

impl ParsedFileInfo {
    /// Create a new ParsedFileInfo
    pub fn new(kind: String, namespace: Option<String>, name: String) -> Self {
        let key = match namespace {
            Some(ns) => format!("{}/{}", ns, name),
            None => name,
        };
        Self { kind, key }
    }
}
