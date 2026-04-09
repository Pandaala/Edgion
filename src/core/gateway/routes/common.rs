use crate::types::resources::common::ParentReference;
use crate::types::resources::http_route::RouteParentStatus;

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
