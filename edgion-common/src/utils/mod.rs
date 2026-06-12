use std::sync::atomic::{AtomicU64, Ordering};

pub mod net;
pub use net::*;

pub mod real_ip_extractor;
pub use real_ip_extractor::{extract_ip_string, RealIpExtractor};

pub mod proxy_protocol;

/// Get the number of available CPU cores on the system
///
/// Returns the number of logical CPU cores available to the current process.
/// This can be used for sizing thread pools, buffers, etc.
pub fn available_cpu_cores() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

static LAST_VERSION: AtomicU64 = AtomicU64::new(0);

/// Generate a unique `resource_version` as a monotonically increasing `u64`.
///
/// The counter is purely in-process and is bumped via `bump_resource_version_floor`
/// during init so that, after restart, the first value returned by this function
/// is strictly greater than any `resource_version` already persisted on disk.
///
/// Safe to call concurrently across threads.
pub fn next_resource_version() -> u64 {
    LAST_VERSION.fetch_add(1, Ordering::Relaxed) + 1
}

/// Raise the `next_resource_version()` floor to at least `rv`.
///
/// Idempotent and concurrent-safe; `rv == 0` is a no-op. Intended to be called
/// once per persisted resource during init, before any external writer is
/// allowed to allocate a new `resource_version`. The admin API enforces this
/// ordering via `require_ready_middleware` (see `controller::api::mod`), which
/// returns 503 until every processor has finished `on_init_done`.
///
/// New write routes that allocate `resource_version` MUST be mounted behind
/// `require_ready_middleware`; otherwise the floor reconstruction can race
/// against external allocation.
pub fn bump_resource_version_floor(rv: u64) {
    if rv > 0 {
        // Relaxed is sufficient: `LAST_VERSION` is self-contained and the
        // init-before-external-write happens-before is provided by the
        // readiness gate, not by this counter.
        LAST_VERSION.fetch_max(rv, Ordering::Relaxed);
    }
}

/// Peek at the last-issued `resource_version` without incrementing it.
///
/// Returns 0 if `next_resource_version()` has never been called. The value is
/// a process-local sequence number (not a wall-clock timestamp). Intended for
/// diagnostic/stats use only — not for assigning versions to resources.
pub fn current_resource_version() -> u64 {
    LAST_VERSION.load(Ordering::Relaxed)
}

/// Resource metadata extracted from YAML/JSON content
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceMetadata {
    pub kind: Option<String>,
    pub namespace: Option<String>,
    pub name: Option<String>,
}

/// Extract kind, namespace, and name from YAML or JSON content without requiring a specific type
///
/// This function parses the content as YAML/JSON and extracts:
/// - `kind` from the top-level `kind` field
/// - `namespace` from `metadata.namespace`
/// - `name` from `metadata.name`
///
/// Returns `None` if the content cannot be parsed, otherwise returns `Some(ResourceMetadata)`
/// with the extracted fields (which may be `None` if not present).
pub fn extract_resource_metadata(content: &str) -> Option<ResourceMetadata> {
    // Try parsing as YAML first
    let value: serde_yaml::Value = match serde_yaml::from_str(content) {
        Ok(v) => v,
        Err(_) => {
            // Fallback to JSON if YAML parsing fails
            serde_json::from_str(content).ok()?
        }
    };

    let kind = value.get("kind").and_then(|v| v.as_str()).map(|s| s.to_string());

    let metadata = value.get("metadata")?;

    let namespace = metadata
        .get("namespace")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let name = metadata.get("name").and_then(|v| v.as_str()).map(|s| s.to_string());

    Some(ResourceMetadata { kind, namespace, name })
}

#[cfg(test)]
mod tests {
    use super::{extract_resource_metadata, next_resource_version};
    use std::thread;

    #[test]
    fn generates_monotonic_versions() {
        let first = next_resource_version();
        let second = next_resource_version();
        assert!(second > first);
    }

    #[test]
    fn concurrent_calls_are_unique_and_ordered() {
        let handles: Vec<_> = (0..64).map(|_| thread::spawn(next_resource_version)).collect();

        let mut values: Vec<u64> = handles
            .into_iter()
            .map(|handle| handle.join().expect("thread panicked"))
            .collect();

        values.sort_unstable();
        values.dedup();
        assert_eq!(values.len(), 64);

        for window in values.windows(2) {
            assert!(window[1] > window[0]);
        }
    }

    #[test]
    fn bump_resource_version_floor_raises_floor() {
        use super::{bump_resource_version_floor, next_resource_version};
        let baseline = next_resource_version();
        let target = baseline + 1_000_000;
        bump_resource_version_floor(target);
        let next = next_resource_version();
        assert!(
            next > target,
            "next_resource_version() should be strictly greater than the bumped floor (next={}, target={})",
            next,
            target
        );
    }

    #[test]
    fn bump_resource_version_floor_no_regression() {
        use super::{bump_resource_version_floor, next_resource_version};
        let baseline = next_resource_version();
        let high = baseline + 2_000_000;
        let low = baseline + 1_000_000;
        bump_resource_version_floor(high);
        bump_resource_version_floor(low);
        let next = next_resource_version();
        assert!(
            next > high,
            "a lower bump must not roll the floor back (next={}, high={})",
            next,
            high
        );
    }

    #[test]
    fn extract_resource_metadata_from_yaml() {
        let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: test-route
  namespace: default
spec:
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
"#;
        let metadata = extract_resource_metadata(yaml).unwrap();
        assert_eq!(metadata.kind, Some("HTTPRoute".to_string()));
        assert_eq!(metadata.namespace, Some("default".to_string()));
        assert_eq!(metadata.name, Some("test-route".to_string()));
    }

    #[test]
    fn extract_resource_metadata_from_json() {
        let json = r#"{
  "apiVersion": "gateway.networking.k8s.io/v1",
  "kind": "GatewayClass",
  "metadata": {
    "name": "test-class"
  }
}"#;
        let metadata = extract_resource_metadata(json).unwrap();
        assert_eq!(metadata.kind, Some("GatewayClass".to_string()));
        assert_eq!(metadata.namespace, None); // Cluster-scoped resource
        assert_eq!(metadata.name, Some("test-class".to_string()));
    }

    #[test]
    fn extract_resource_metadata_missing_fields() {
        let yaml = r#"
apiVersion: v1
kind: Service
metadata:
  name: test-service
"#;
        let metadata = extract_resource_metadata(yaml).unwrap();
        assert_eq!(metadata.kind, Some("Service".to_string()));
        assert_eq!(metadata.namespace, None);
        assert_eq!(metadata.name, Some("test-service".to_string()));
    }

    #[test]
    fn extract_resource_metadata_invalid_content() {
        let invalid = "not yaml or json content";
        assert!(extract_resource_metadata(invalid).is_none());
    }
}
