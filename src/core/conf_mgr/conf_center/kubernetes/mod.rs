//! Kubernetes configuration center module
//!
//! Uses kube-runtime Controller pattern for event-driven reconciliation.
//! Each resource type runs as an independent ResourceController with its own
//! complete 1-8 lifecycle (create store, reflector, wait ready, init, reconcile loop).

mod context;
mod controller;
mod error;
mod namespace;
mod reconcilers;
mod resource_controller;
mod writer;

pub use controller::KubernetesController;
pub use context::ControllerContext;
pub use error::ReconcileError;
pub use namespace::NamespaceWatchMode;
pub use resource_controller::{ResourceController, ResourceControllerBuilder};
pub use writer::KubernetesWriter;

// Re-export status types from conf_center::status
pub mod status {
    pub use super::super::status::{KubernetesStatusStore, StatusStore, StatusStoreError};
}

pub use status::{KubernetesStatusStore, StatusStore, StatusStoreError};
