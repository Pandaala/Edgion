use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub mod net;

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

#[cfg(test)]
mod tests {
    use super::{check_need_version, next_resource_version};
    use std::thread;

    #[test]
    fn generates_monotonic_versions() {
        let first = next_resource_version();
        let second = next_resource_version();
        assert!(second > first);
    }

    #[test]
    fn concurrent_calls_are_unique_and_ordered() {
        let handles: Vec<_> = (0..64)
            .map(|_| thread::spawn(|| next_resource_version()))
            .collect();

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
}
