//! Resolved runtime target for outbound HTTP calls.
//!
//! [`ResolvedTarget`] is the canonical target representation for the shared
//! outbound-call engine.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::backends::discovery::endpoint_slice::get_roundrobin_store;

/// Resolved target for an outbound HTTP call at runtime.
///
/// `Static` covers `Uri` and `ServiceClusterIp` (a fixed base URL). `Service`
/// holds a reference into the shared EndpointSlice data layer plus a local
/// round-robin cursor; endpoints are read fresh on every `pick_url()`, so pod
/// churn never rebuilds this target.
pub enum ResolvedTarget {
    /// Single base URL: Uri target or ServiceClusterIp.
    Static(String),
    /// Service-backed pool read from the shared EpSliceStore at call time.
    Service {
        /// Canonical `"namespace/service-name"` key (matches EpSliceStore).
        service_key: String,
        /// Kubernetes Service port number; a missing or invalid value fails closed.
        port: Option<i32>,
        /// "http" or "https".
        scheme: &'static str,
        /// Local round-robin cursor.
        cursor: AtomicUsize,
    },
}

impl std::fmt::Debug for ResolvedTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Static(url) => write!(f, "Static({url:?})"),
            Self::Service {
                service_key,
                port,
                scheme,
                ..
            } => write!(f, "Service(key={service_key:?}, port={port:?}, scheme={scheme})"),
        }
    }
}

impl ResolvedTarget {
    /// Pick the next base URL. Returns `None` when the Service target currently
    /// has no ready backend.
    pub fn pick_url(&self) -> Option<String> {
        match self {
            Self::Static(url) => Some(url.clone()),
            Self::Service {
                service_key,
                port: service_port,
                scheme,
                cursor,
            } => {
                let eps = get_roundrobin_store().ready_endpoints_for_port(service_key, *service_port);
                rr_pick(&eps, cursor, scheme)
            }
        }
    }
}

/// Round-robin one `"ip:port"` from `eps` and format `"{scheme}://{ep}"`.
/// Returns `None` for an empty list. Pure (no global state) for testability.
pub fn rr_pick(eps: &[String], cursor: &AtomicUsize, scheme: &str) -> Option<String> {
    if eps.is_empty() {
        return None;
    }
    let idx = cursor.fetch_add(1, Ordering::Relaxed) % eps.len();
    Some(format!("{scheme}://{}", eps[idx]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rr_pick_rotates_canonical_service_endpoints_and_handles_empty() {
        let cursor = AtomicUsize::new(0);
        let eps = vec!["10.0.0.1:8443".to_string(), "[2001:db8::1]:9443".to_string()];

        let a = rr_pick(&eps, &cursor, "http");
        let b = rr_pick(&eps, &cursor, "https");
        let c = rr_pick(&eps, &cursor, "http");
        assert_eq!(a.as_deref(), Some("http://10.0.0.1:8443"));
        assert_eq!(b.as_deref(), Some("https://[2001:db8::1]:9443"));
        assert_eq!(c.as_deref(), Some("http://10.0.0.1:8443"));

        let empty: Vec<String> = vec![];
        assert_eq!(rr_pick(&empty, &cursor, "http"), None);
    }
}
