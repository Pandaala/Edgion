//! Error types and policies for Kubernetes controller

use kube::runtime::controller::Action;
use kube::Resource;
use std::sync::Arc;
use std::time::Duration;

use super::context::ControllerContext;

/// Reconcile error type
#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    #[error("Kube error: {0}")]
    KubeError(#[from] kube::Error),
    #[error("Other error: {0}")]
    Other(String),
}

/// Generic error policy function for all controllers
pub fn error_policy<K: Resource>(
    _obj: Arc<K>,
    error: &ReconcileError,
    _ctx: Arc<ControllerContext>,
) -> Action {
    tracing::warn!(error = %error, "Reconcile error, will retry");
    Action::requeue(Duration::from_secs(30))
}
