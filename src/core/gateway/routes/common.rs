use crate::types::constants::annotations::edgion::EDGION_STREAM_PLUGINS;
use crate::types::resources::common::ParentReference;
use crate::types::resources::http_route::RouteParentStatus;
use kube::api::ObjectMeta;

/// Canonical access-log string for an upstream address rejected by the
/// SSRF guard (`validate_endpoint_backend_addr`). Centralized so the
/// per-protocol fail-close paths (TCP, TLS terminate, TLS passthrough, UDP,
/// HTTP) emit the same literal and operator log-scraping rules match across
/// all protocols. Update here only; do not re-declare in protocol modules.
pub(crate) const UNSAFE_UPSTREAM_ADDRESS_LOG: &str = "unsafe upstream address";

/// Check if a specific parentRef has Accepted=True in the route status.
///
/// Returns true when no matching status entry is found (optimistic: include
/// until the controller explicitly rejects it, e.g. during the first sync
/// before status is populated).
fn is_parent_ref_accepted(pr: &ParentReference, status_parents: &[RouteParentStatus], route_ns: Option<&str>) -> bool {
    let pr_ns = pr.namespace.as_deref().or(route_ns).unwrap_or("default");

    for sp in status_parents {
        let sp_ns = sp.parent_ref.namespace.as_deref().or(route_ns).unwrap_or("default");
        if sp_ns == pr_ns
            && sp.parent_ref.name == pr.name
            && sp.parent_ref.section_name == pr.section_name
            && sp.parent_ref.port == pr.port
        {
            return sp
                .conditions
                .iter()
                .any(|c| c.type_ == "Accepted" && c.status == "True");
        }
    }
    true
}

/// Filter parentRefs to only those with Accepted=True status.
///
/// Routes whose parentRef is not accepted by any Gateway/Listener are excluded
/// from compilation, keeping the route table clean and simplifying debugging.
///
/// Returns None if no parentRefs survive (route should be skipped entirely).
/// When status is not yet available, all parentRefs are included optimistically.
pub(crate) fn filter_accepted_parent_refs(
    parent_refs: Option<&[ParentReference]>,
    status_parents: Option<&[RouteParentStatus]>,
    route_ns: Option<&str>,
) -> Option<Vec<ParentReference>> {
    let parent_refs = parent_refs?;
    if parent_refs.is_empty() {
        return None;
    }

    let status_parents = match status_parents {
        Some(sp) if !sp.is_empty() => sp,
        _ => return Some(parent_refs.to_vec()),
    };

    let accepted: Vec<ParentReference> = parent_refs
        .iter()
        .filter(|pr| is_parent_ref_accepted(pr, status_parents, route_ns))
        .cloned()
        .collect();

    if accepted.is_empty() {
        None
    } else {
        Some(accepted)
    }
}

/// Return the effective hostnames for a route.
///
/// Priority:
/// 1. `resolved` — pre-computed by the controller (intersection with listener)
/// 2. `spec` — route's own hostname list (fallback for legacy/bootstrap)
/// 3. `"*"` — wildcard when no hostnames specified
pub(crate) fn effective_hostnames(resolved: Option<&[String]>, spec: Option<&[String]>) -> Vec<String> {
    if let Some(r) = resolved {
        if !r.is_empty() {
            return r.to_vec();
        }
    }
    if let Some(s) = spec {
        if !s.is_empty() {
            return s.to_vec();
        }
    }
    vec!["*".to_string()]
}

/// A BackendRef is routable only when it targets a core Kubernetes Service:
/// group is `""` / `"core"` / omitted and kind is `"Service"` / omitted.
///
/// Single source of truth shared by all L4 route modules (TCP, TLS termination,
/// TLS passthrough, UDP). Struct-based callers pass
/// `br.group.as_deref(), br.kind.as_deref()`.
pub(crate) fn is_core_service_backend_ref(group: Option<&str>, kind: Option<&str>) -> bool {
    matches!(group, None | Some("") | Some("core")) && matches!(kind, None | Some("Service"))
}

/// Resolve the stream plugin store key from a route's metadata annotations.
///
/// Reads the `EDGION_STREAM_PLUGINS` annotation, trims it, and returns:
/// - `None` if the annotation is absent or whitespace-only.
/// - The trimmed value as-is when it already contains `/` (treated as a
///   fully-qualified `namespace/name` reference).
/// - `<route-namespace>/<trimmed>` otherwise, falling back to `default` when
///   the route has no namespace.
///
/// Single source of truth for the L4 stream-plugin annotation contract; shared
/// by TCP, TLS, and UDP route handlers so future edits to the normalization
/// rule do not have to be repeated per route type.
pub(crate) fn resolve_stream_plugin_store_key(metadata: &ObjectMeta) -> Option<String> {
    let annotations = metadata.annotations.as_ref()?;
    let annotation_value = annotations.get(EDGION_STREAM_PLUGINS)?;
    let trimmed = annotation_value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let store_key = if trimmed.contains('/') {
        trimmed.to_string()
    } else {
        let namespace = metadata.namespace.as_deref().unwrap_or("default");
        format!("{}/{}", namespace, trimmed)
    };

    Some(store_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn is_core_service_backend_ref_accepts_core_service_and_rejects_others() {
        // Omitted group + kind defaults to a core Service.
        assert!(is_core_service_backend_ref(None, None));
        // Explicit empty / "core" group with the Service kind.
        assert!(is_core_service_backend_ref(Some(""), Some("Service")));
        assert!(is_core_service_backend_ref(Some("core"), Some("Service")));
        // Unknown group or non-Service kind is not routable.
        assert!(!is_core_service_backend_ref(Some("example.com"), Some("Foo")));
        assert!(!is_core_service_backend_ref(Some(""), Some("Deployment")));
    }

    fn meta_with(namespace: Option<&str>, annotation_value: Option<&str>) -> ObjectMeta {
        let annotations = annotation_value.map(|v| {
            let mut a = BTreeMap::new();
            a.insert(EDGION_STREAM_PLUGINS.to_string(), v.to_string());
            a
        });
        ObjectMeta {
            namespace: namespace.map(str::to_string),
            annotations,
            ..Default::default()
        }
    }

    #[test]
    fn resolve_stream_plugin_store_key_returns_none_when_annotation_missing() {
        let meta = meta_with(Some("ns-a"), None);
        assert_eq!(resolve_stream_plugin_store_key(&meta), None);
    }

    #[test]
    fn resolve_stream_plugin_store_key_returns_none_when_whitespace_only() {
        let meta = meta_with(Some("ns-a"), Some("   \t  "));
        assert_eq!(resolve_stream_plugin_store_key(&meta), None);
    }

    #[test]
    fn resolve_stream_plugin_store_key_preserves_already_qualified_key() {
        let meta = meta_with(Some("ns-a"), Some("  other-ns/plugins-1  "));
        assert_eq!(
            resolve_stream_plugin_store_key(&meta),
            Some("other-ns/plugins-1".to_string())
        );
    }

    #[test]
    fn resolve_stream_plugin_store_key_prepends_route_namespace_for_bare_key() {
        let meta = meta_with(Some("ns-a"), Some("plugins-1"));
        assert_eq!(
            resolve_stream_plugin_store_key(&meta),
            Some("ns-a/plugins-1".to_string())
        );
    }

    #[test]
    fn resolve_stream_plugin_store_key_falls_back_to_default_namespace() {
        let meta = meta_with(None, Some("plugins-1"));
        assert_eq!(
            resolve_stream_plugin_store_key(&meta),
            Some("default/plugins-1".to_string())
        );
    }
}
