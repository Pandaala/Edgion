//! Kubernetes based configuration center
//!
//! Provides:
//! - KubernetesWriter: ConfWriter implementation using K8s API
//! - KubernetesStore: In-memory cache updated by Controller
//! - KubernetesController: Watches K8s resources and updates cache/ConfigServer

mod controller;
mod reconciler;
mod store;
mod writer;

pub use controller::KubernetesController;
pub use reconciler::StatusReconciler;
pub use store::KubernetesStore;
pub use writer::KubernetesWriter;
