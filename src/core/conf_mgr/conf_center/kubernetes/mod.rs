//! Kubernetes configuration center module
//!
//! Uses kube-runtime Controller pattern for event-driven reconciliation.

mod context;
mod controller;
mod error;
mod namespace;
mod reconcilers;
mod writer;

pub use controller::KubernetesController;
pub use context::ControllerContext;
pub use error::ReconcileError;
pub use namespace::NamespaceWatchMode;
pub use writer::KubernetesWriter;

// Re-export status types from conf_center::status
pub mod status {
    pub use super::super::status::{KubernetesStatusStore, StatusStore, StatusStoreError};
}

pub use status::{KubernetesStatusStore, StatusStore, StatusStoreError};
