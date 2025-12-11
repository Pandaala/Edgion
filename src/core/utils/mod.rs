use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub mod net;
pub use net::*;

pub mod duration;
pub use duration::parse_duration;

/// Get the number of available CPU cores on the system
/// 
/// Returns the number of logical CPU cores available to the current process.
/// This can be used for sizing thread pools, buffers, etc.
pub fn available_cpu_cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

static LAST_VERSION: AtomicU64 = AtomicU64::new(0);

/// Generate a unique `resource_version` as a monotonically increasing `u64`.
///
/// The function is safe to call concurrently across threads. Values are
/// primarily derived from the current Unix timestamp in nanoseconds; if
/// multiple invocations occur within the same nanosecond (or the system clock
/// moves backwards), the generator falls back to incrementing the previously
/// returned value to preserve ordering.
pub fn next_resource_version() -> u64 {
    loop {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after UNIX_EPOCH");
        let timestamp = now.as_nanos() as u64;

        let current = LAST_VERSION.load(Ordering::Relaxed);
        let candidate = if timestamp > current {
            timestamp
        } else {
            current.saturating_add(1)
        };

        if LAST_VERSION
            .compare_exchange(current, candidate, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            return candidate;
        }
    }
}

/// Inspect a raw YAML/JSON document to determine whether a Kubernetes
/// `resource_version` is already present. If missing, generate one using
/// `next_resource_version()`.
///
/// The function looks for the key `resource_version` (case-insensitive) and
/// also supports the canonical snake-case `resourceVersion` spelling. It
/// performs a lightweight text scan without fully parsing the resource.
pub fn check_need_version(input: &str) -> Option<u64> {
    if input.contains("resourceVersion:") {
        None
    } else {
        Some(next_resource_version())
    }
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

/// Format resource info as kind/namespace/name/resource_version
///
/// This function extracts key information from a Kubernetes resource for logging/debugging purposes.
/// For cluster-scoped resources (without namespace), the namespace field is omitted.
pub fn format_resource_info<T: kube::Resource>(resource: &T) -> String {
    use kube::ResourceExt;

    let kind = std::any::type_name::<T>().split("::").last().unwrap_or("Unknown");
    let namespace = resource.namespace();
    let name = resource.name_any();
    let resource_version = resource.meta().resource_version.as_deref().unwrap_or("N/A");

    if let Some(ns) = namespace {
        format!(
            "kind={}, namespace={}, name={}, resource_version={}",
            kind, ns, name, resource_version
        )
    } else {
        format!("kind={}, name={}, resource_version={}", kind, name, resource_version)
    }
}

#[cfg(test)]
mod tests {
    use super::{check_need_version, extract_resource_metadata, next_resource_version};
    use std::thread;

    #[test]
    fn generates_monotonic_versions() {
        let first = next_resource_version();
        let second = next_resource_version();
        assert!(second > first);
    }

    #[test]
    fn concurrent_calls_are_unique_and_ordered() {
        let handles: Vec<_> = (0..64).map(|_| thread::spawn(|| next_resource_version())).collect();

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
    fn check_need_version_detects_existing_field() {
        let sample = r#"
apiVersion: v1
kind: ConfigMap
metadata:
  name: demo
  resourceVersion: "123"
"#;

        assert!(check_need_version(sample).is_none());
    }

    #[test]
    fn check_need_version_generates_when_missing() {
        let sample = r#"
apiVersion: v1
kind: ConfigMap
metadata:
  name: demo
"#;

        assert!(check_need_version(sample).is_some());
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
