//! Error types and policies for Kubernetes controller

use kube::runtime::controller::Action;
use kube::Resource;
use rand::Rng;
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

/// Base retry intervals for different error types
const RETRY_BASE_TRANSIENT: u64 = 5;   // Transient errors (connection, timeout)
const RETRY_BASE_NOT_FOUND: u64 = 30;  // Resource not found
const RETRY_BASE_CONFLICT: u64 = 2;    // Conflict errors (retry quickly)
const RETRY_BASE_OTHER: u64 = 30;      // Other errors

/// Maximum jitter to add (in seconds)
const RETRY_MAX_JITTER: u64 = 10;

/// Generic error policy function for all controllers
///
/// Uses different retry intervals based on error type:
/// - Transient errors (connection, timeout): 5s base
/// - Conflict errors: 2s base (retry quickly)
/// - Not found: 30s base
/// - Other errors: 30s base
///
/// Adds random jitter (0-10s) to avoid thundering herd.
pub fn error_policy<K: Resource>(
    _obj: Arc<K>,
    error: &ReconcileError,
    _ctx: Arc<ControllerContext>,
) -> Action {
    let base_secs = match error {
        ReconcileError::KubeError(kube_err) => {
            // Categorize kube errors
            match kube_err {
                kube::Error::Api(api_err) => {
                    match api_err.code {
                        404 => RETRY_BASE_NOT_FOUND,      // Not found
                        409 => RETRY_BASE_CONFLICT,       // Conflict
                        429 => RETRY_BASE_TRANSIENT * 2,  // Rate limited - longer wait
                        500..=599 => RETRY_BASE_TRANSIENT, // Server errors
                        _ => RETRY_BASE_OTHER,
                    }
                }
                // Connection/timeout errors - transient
                kube::Error::HyperError(_)
                | kube::Error::Service(_) => RETRY_BASE_TRANSIENT,
                _ => RETRY_BASE_OTHER,
            }
        }
        ReconcileError::Other(_) => RETRY_BASE_OTHER,
    };

    // Add random jitter to avoid thundering herd
    let jitter = rand::thread_rng().gen_range(0..=RETRY_MAX_JITTER);
    let total_secs = base_secs + jitter;

    tracing::warn!(
        error = %error,
        retry_secs = total_secs,
        "Reconcile error, will retry with jitter"
    );

    Action::requeue(Duration::from_secs(total_secs))
}
