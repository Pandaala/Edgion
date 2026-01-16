//! Status storage module
//!
//! Provides abstractions for storing resource status across different backends:
//! - Kubernetes: Updates status via K8s API (Server-Side Apply)
//! - FileSystem: Persists status to local JSON files

mod file_system;
mod kubernetes;
mod traits;

pub use file_system::FileSystemStatusStore;
pub use kubernetes::KubernetesStatusStore;
pub use traits::{StatusStore, StatusStoreError};
