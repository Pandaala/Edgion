//! Circuit breaker — admit-time per-endpoint concurrency cap enforcement.
//!
//! The counter increment IS the admission: [`CbPermit::try_acquire`] atomically
//! reserves one in-flight slot on the chosen endpoint BEFORE the upstream
//! connect. The slot is returned exactly once, automatically, when the permit is
//! dropped — the permit is held on the request context, so every
//! request-termination path (success, upstream error, client disconnect,
//! cancellation, early return) releases the slot without an explicit call.

use crate::lb::edgion_backend::EdgionBackend;
use std::sync::Arc;

/// RAII circuit-breaker permit. Acquired at admit time; releasing is automatic
/// on drop, so every request-termination path (success, upstream error, client
/// disconnect, cancellation, early return) returns the slot exactly once.
pub struct CbPermit {
    backend: Arc<EdgionBackend>,
}

impl CbPermit {
    /// Try to admit one request to `backend` under cap `max`. Returns the held
    /// permit on success, or `None` if the endpoint is at cap (caller must shed).
    pub fn try_acquire(backend: Arc<EdgionBackend>, max: u32) -> Option<CbPermit> {
        if backend.try_acquire_cb_permit(max) {
            Some(CbPermit { backend })
        } else {
            None
        }
    }
}

impl Drop for CbPermit {
    fn drop(&mut self) {
        self.backend.release_cb_permit();
    }
}

/// Record a shed (endpoint at cap) for the given Service key. Increments
/// `edgion_gateway_circuit_breaker_shed_total`, labelled by `service` only
/// (never per-endpoint or per-request — see [`try_acquire`](CbPermit::try_acquire)).
pub fn inc_shed_total(service_key: &str) {
    crate::observe::metrics::registry::record_cb_shed(service_key);
}

#[cfg(test)]
mod concurrency_integration_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use pingora_load_balancing::Backend;

    #[test]
    fn permit_releases_on_drop() {
        let b = Arc::new(EdgionBackend::new(Backend::new("127.0.0.1:1").unwrap()));
        {
            let _p = CbPermit::try_acquire(b.clone(), 1).unwrap();
            assert_eq!(b.cb_inflight(), 1);
            assert!(CbPermit::try_acquire(b.clone(), 1).is_none()); // at cap
        }
        assert_eq!(b.cb_inflight(), 0); // dropped -> released
    }
}
