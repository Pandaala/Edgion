use crate::runtime::matching::route::ResolvedParentRef;
use crate::runtime::store::listener_gateway_info::format_gateway_key;
use edgion_resources::constants::annotations::edgion::EDGION_STREAM_PLUGINS;
use edgion_resources::constants::annotations::edgion::{
    TCP_KEEPALIVE_INTERVAL, TCP_KEEPALIVE_PROBES, TCP_KEEPALIVE_TIME,
};
use edgion_resources::resources::common::{ParentReference, RefDenied, TcpKeepaliveConfig};
use edgion_resources::resources::http_route::RouteParentStatus;
use kube::api::ObjectMeta;
use std::collections::BTreeMap;
use std::time::Duration;

/// Canonical access-log string for an upstream address rejected by the
/// SSRF guard (`validate_endpoint_backend_addr`). Centralized so the
/// per-protocol fail-close paths (TCP, TLS terminate, TLS passthrough, UDP,
/// HTTP) emit the same literal and operator log-scraping rules match across
/// all protocols. Update here only; do not re-declare in protocol modules.
pub(crate) const UNSAFE_UPSTREAM_ADDRESS_LOG: &str = "unsafe upstream address";

/// TCP `status` / UDP failure-status token when a selected L4 backendRef
/// carries Controller-stamped `ref_denied`. Fixed and non-identifying: do
/// not interpolate grant, namespace, or target identity into this token.
pub(crate) const L4_BACKEND_REF_DENIED_STATUS: &str = "RefDenied";

/// TLS `err_log` / TCP `deny_reason` literal for the same authorization
/// miss. Same sensitivity rule as `L4_BACKEND_REF_DENIED_STATUS`.
pub(crate) const L4_BACKEND_REF_DENIED_LOG: &str = "backend ref denied";

/// TCP `status` / UDP failure-status / TLS `status` token when a selected
/// L4 backendRef is not a core Service. Fixed and non-identifying.
pub(crate) const L4_BACKEND_UNSUPPORTED_KIND_STATUS: &str = "UnsupportedBackend";

/// TLS `err_log` / TCP `deny_reason` literal for an unsupported L4
/// backend kind. Do not interpolate group, kind, or name.
pub(crate) const L4_BACKEND_UNSUPPORTED_KIND_LOG: &str = "unsupported backend kind";

/// Outcome of the shared L4 post-select gate.
///
/// `Denied` and `UnsupportedKind` are terminal: the connection or new
/// session must end without Service lookup, EndpointSlice resolution, or
/// dialing, and TLS must not retry onto another backendRef.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum L4SelectedBackend {
    Denied,
    UnsupportedKind,
    Proceed,
}

/// Final pre-dial authorization and kind gate for a selected L4 backendRef.
///
/// Controller-stamped `ref_denied` wins over kind. Preload filtering of
/// denied or unsupported refs is a startup optimization only and is not
/// this boundary. Denied and unsupported slots keep their declared WRR
/// weight and fail after selection.
#[inline]
pub(crate) fn classify_l4_selected_backend(
    ref_denied: Option<&RefDenied>,
    group: Option<&str>,
    kind: Option<&str>,
) -> L4SelectedBackend {
    if ref_denied.is_some() {
        L4SelectedBackend::Denied
    } else if !is_core_service_backend_ref(group, kind) {
        L4SelectedBackend::UnsupportedKind
    } else {
        L4SelectedBackend::Proceed
    }
}

/// Shared post-select action for every L4 dial path, including TLS retry
/// loops. There is no retry variant: authorization and kind misses must
/// `return`, never `continue`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum L4PostSelect {
    FailClosed { status: &'static str, log: &'static str },
    Proceed,
}

/// Map a selected L4 backendRef to the shared dial-path action.
#[inline]
pub(crate) fn l4_post_select(ref_denied: Option<&RefDenied>, group: Option<&str>, kind: Option<&str>) -> L4PostSelect {
    match classify_l4_selected_backend(ref_denied, group, kind) {
        L4SelectedBackend::Denied => L4PostSelect::FailClosed {
            status: L4_BACKEND_REF_DENIED_STATUS,
            log: L4_BACKEND_REF_DENIED_LOG,
        },
        L4SelectedBackend::UnsupportedKind => L4PostSelect::FailClosed {
            status: L4_BACKEND_UNSUPPORTED_KIND_STATUS,
            log: L4_BACKEND_UNSUPPORTED_KIND_LOG,
        },
        L4SelectedBackend::Proceed => L4PostSelect::Proceed,
    }
}

/// Convert the shared, runtime-agnostic keepalive configuration at the
/// data-plane boundary. `user_timeout = 0` keeps the Linux system default.
pub(crate) fn to_pingora_tcp_keepalive(config: &TcpKeepaliveConfig) -> pingora_core::protocols::l4::ext::TcpKeepalive {
    pingora_core::protocols::l4::ext::TcpKeepalive {
        idle: config.idle,
        interval: config.interval,
        count: config.count,
        #[cfg(target_os = "linux")]
        user_timeout: std::time::Duration::ZERO,
    }
}

/// Check the stamped controller's acceptance for one canonical parent identity.
///
/// Returns true when no matching status entry is found (optimistic: include
/// until the controller explicitly rejects it, e.g. during the first sync
/// before status is populated).
fn is_parent_ref_accepted(
    pr: &ParentReference,
    status_parents: &[RouteParentStatus],
    route_ns: Option<&str>,
    expected_controller: &str,
) -> bool {
    let identity = pr.canonical_identity(route_ns);
    let mut owned = status_parents.iter().filter(|status| {
        let status_identity = status.canonical_identity(route_ns);
        status_identity.parent == identity && status_identity.controller_name == expected_controller
    });
    let Some(status) = owned.next() else {
        return true;
    };
    // A processed Controller generation deduplicates owned entries. Reject
    // ambiguous snapshots independently of list order if that contract is broken.
    owned.next().is_none()
        && status
            .conditions
            .iter()
            .any(|c| c.type_ == "Accepted" && c.status == "True")
}

#[inline]
fn is_gateway_parent_ref(pr: &ParentReference) -> bool {
    pr.canonical_identity(None).is_gateway()
}

/// Filter Gateway parentRefs using the processed generation's status owner.
///
/// An existing owned status must be uniquely identified and Accepted=True.
///
/// Returns None if no parentRefs survive (route should be skipped entirely).
/// A nonempty Controller-owned transport stamp is required. With that stamp,
/// missing owned status retains optimistic acceptance; foreign status is ignored.
pub(crate) fn filter_accepted_parent_refs(
    parent_refs: Option<&[ParentReference]>,
    status_parents: Option<&[RouteParentStatus]>,
    route_ns: Option<&str>,
    expected_controller: Option<&str>,
) -> Option<Vec<ParentReference>> {
    let expected_controller = expected_controller.filter(|owner| !owner.is_empty())?;
    let parent_refs = parent_refs?;
    if parent_refs.is_empty() {
        return None;
    }

    let accepted: Vec<ParentReference> = parent_refs
        .iter()
        .filter(|pr| {
            is_gateway_parent_ref(pr)
                && match status_parents {
                    Some(sp) if !sp.is_empty() => is_parent_ref_accepted(pr, sp, route_ns, expected_controller),
                    _ => true,
                }
        })
        .cloned()
        .collect();

    if accepted.is_empty() {
        None
    } else {
        Some(accepted)
    }
}

/// Extract `(gateway_ns, gateway_name)` pairs from accepted parent_refs.
/// Filters by both `kind == "Gateway"` and `group == "gateway.networking.k8s.io"`
/// using Gateway API defaults: an absent group defaults to
/// `gateway.networking.k8s.io` and an absent kind defaults to `Gateway`. Falls
/// back to `route_ns` when a parent ref omits its namespace.
pub(crate) fn gateways_from_parent_refs(parent_refs: &[ParentReference], route_ns: &str) -> Vec<(String, String)> {
    parent_refs
        .iter()
        .filter(|pr| is_gateway_parent_ref(pr))
        .map(|pr| {
            let ns = pr.namespace.clone().unwrap_or_else(|| route_ns.to_string());
            (ns, pr.name.clone())
        })
        .collect()
}

/// Resolve accepted parentRefs into the form the request path consumes.
///
/// Runs on the config-publish path so `check_gateway_listener_match` never has
/// to build its `GatewayInfoIndex` lookup key per request per route candidate.
///
/// Production HTTP and gRPC config-publication callers pass parentRefs already
/// accepted as Gateway references. This function remains a generic mapper and
/// deliberately does not inspect `kind` or `group` itself.
///
/// Order is preserved: `check_gateway_listener_match` returns the first match,
/// so reordering would change which Gateway a route attaches to.
pub(crate) fn resolve_parent_refs(parent_refs: &[ParentReference], route_ns: &str) -> Vec<ResolvedParentRef> {
    parent_refs
        .iter()
        .map(|pr| {
            let parent_ns = pr.namespace.as_deref().unwrap_or(route_ns);
            ResolvedParentRef {
                gateway_key: format_gateway_key(parent_ns, &pr.name),
                parent_ns: parent_ns.to_string(),
                section_name: pr.section_name.clone(),
                port: pr.port,
            }
        })
        .collect()
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

/// Gateway stream attachments are same-namespace references. Route attachments
/// use the separate resolver above and may be explicitly namespace-qualified.
pub(crate) fn resolve_gateway_stream_plugin_store_key(value: &str, gateway_ns: &str) -> Result<Option<String>, ()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else if trimmed.contains('/') {
        Err(())
    } else {
        Ok(Some(format!("{}/{}", gateway_ns, trimmed)))
    }
}

/// Inclusive bounds for the keepalive annotation values. Out-of-range inputs
/// are clamped (not rejected) with a WARN, mirroring `cap_tls_connect_retries`.
///
/// Note: the `_MIN` lower bounds (all 1) are unreachable in practice — the
/// `> 0` filter on the parsed `i64` already guarantees every value reaching
/// `clamp_warn_*` is >= 1. The constants are kept for documentation and
/// symmetry only.
const KEEPALIVE_TIME_MIN_SECS: u64 = 1;
const KEEPALIVE_TIME_MAX_SECS: u64 = 7200;
const KEEPALIVE_INTERVAL_MIN_SECS: u64 = 1;
const KEEPALIVE_INTERVAL_MAX_SECS: u64 = 75;
const KEEPALIVE_PROBES_MIN: usize = 1;
const KEEPALIVE_PROBES_MAX: usize = 20;

/// OS-default fallbacks (Linux `TCP_KEEPINTVL` / `TCP_KEEPCNT`) used when only
/// the enabling `time` annotation is present. Pingora pushes all of
/// idle/interval/count to setsockopt unconditionally once keepalive is enabled,
/// so we must supply concrete values here rather than relying on the kernel.
// Intentionally equals KEEPALIVE_INTERVAL_MAX_SECS (75s) — the OS default
// matches the ceiling, so this is not a copy-paste mistake.
const KEEPALIVE_INTERVAL_DEFAULT_SECS: u64 = 75;
const KEEPALIVE_PROBES_DEFAULT: usize = 9;

fn clamp_warn_secs(v: u64, min: u64, max: u64, route_key: &str, field: &str) -> u64 {
    let clamped = v.clamp(min, max);
    if clamped != v {
        tracing::warn!(route = %route_key, field, requested = v, clamped, "tcp keepalive value clamped");
    }
    clamped
}

fn clamp_warn_count(v: usize, min: usize, max: usize, route_key: &str, field: &str) -> usize {
    let clamped = v.clamp(min, max);
    if clamped != v {
        tracing::warn!(route = %route_key, field, requested = v, clamped, "tcp keepalive value clamped");
    }
    clamped
}

/// Resolve the `edgion.io/tcp-keepalive-*` annotations into a keepalive config.
///
/// Returns `None` (keepalive disabled — current default behavior) unless
/// `tcp-keepalive-time` is present and parses to a positive integer. When
/// enabled, `interval` and `probes` fall back to the OS defaults (75s / 9) if
/// absent/invalid, and every field is clamped to a sane range with a WARN.
///
/// Single source of truth shared by the TCP and TLS conf-handlers.
pub fn resolve_tcp_keepalive(
    annotations: Option<&BTreeMap<String, String>>,
    route_key: &str,
) -> Option<TcpKeepaliveConfig> {
    let annotations = annotations?;

    // Enablement trigger: time present and > 0.
    let raw_time = annotations.get(TCP_KEEPALIVE_TIME)?.trim().parse::<i64>().ok()?;
    if raw_time <= 0 {
        return None;
    }
    let time = clamp_warn_secs(
        raw_time as u64,
        KEEPALIVE_TIME_MIN_SECS,
        KEEPALIVE_TIME_MAX_SECS,
        route_key,
        "time",
    );

    let interval = annotations
        .get(TCP_KEEPALIVE_INTERVAL)
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|v| *v > 0)
        .map(|v| {
            clamp_warn_secs(
                v as u64,
                KEEPALIVE_INTERVAL_MIN_SECS,
                KEEPALIVE_INTERVAL_MAX_SECS,
                route_key,
                "interval",
            )
        })
        .unwrap_or(KEEPALIVE_INTERVAL_DEFAULT_SECS);

    let count = annotations
        .get(TCP_KEEPALIVE_PROBES)
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|v| *v > 0)
        .map(|v| {
            clamp_warn_count(
                v as usize,
                KEEPALIVE_PROBES_MIN,
                KEEPALIVE_PROBES_MAX,
                route_key,
                "probes",
            )
        })
        .unwrap_or(KEEPALIVE_PROBES_DEFAULT);

    Some(TcpKeepaliveConfig {
        idle: Duration::from_secs(time),
        interval: Duration::from_secs(interval),
        count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::common::Condition;
    use edgion_resources::resources::http_route::GATEWAY_API_GROUP;
    use std::collections::BTreeMap;

    fn parent_ref(name: &str, namespace: Option<&str>) -> ParentReference {
        ParentReference {
            group: None,
            kind: None,
            namespace: namespace.map(str::to_string),
            name: name.to_string(),
            section_name: None,
            port: None,
        }
    }

    fn parent_status(parent_ref: ParentReference, accepted: bool) -> RouteParentStatus {
        RouteParentStatus {
            parent_ref,
            controller_name: "edgion.io/gateway-controller".to_string(),
            conditions: vec![Condition {
                last_transition_time: "2026-09-08T00:00:00Z".to_string(),
                message: String::new(),
                observed_generation: Some(1),
                reason: "Test".to_string(),
                status: if accepted { "True" } else { "False" }.to_string(),
                type_: "Accepted".to_string(),
            }],
        }
    }

    #[test]
    fn parent_acceptance_requires_transport_owner_and_ignores_foreign_status() {
        let refs = vec![parent_ref("gw", None)];
        let mut own = parent_status(refs[0].clone(), false);
        own.controller_name = "example.net/custom-controller".into();
        let foreign = parent_status(refs[0].clone(), true);
        for owner in [None, Some("")] {
            assert!(filter_accepted_parent_refs(Some(&refs), None, Some("apps"), owner).is_none());
        }
        for accepted in [false, true] {
            own.conditions[0].status = if accepted { "True" } else { "False" }.into();
            let mut foreign = foreign.clone();
            foreign.conditions[0].status = if accepted { "False" } else { "True" }.into();
            for statuses in [vec![own.clone(), foreign.clone()], vec![foreign, own.clone()]] {
                assert_eq!(
                    filter_accepted_parent_refs(Some(&refs), Some(&statuses), Some("apps"), Some(&own.controller_name))
                        .is_some(),
                    accepted
                );
            }
        }
        for statuses in [None, Some(vec![]), Some(vec![foreign])] {
            assert_eq!(
                filter_accepted_parent_refs(
                    Some(&refs),
                    statuses.as_deref(),
                    Some("apps"),
                    Some(&own.controller_name)
                ),
                Some(refs.clone())
            );
        }
    }

    #[test]
    fn duplicate_owned_status_is_rejected_even_with_equivalent_default_spelling() {
        let refs = vec![parent_ref("gw", None)];
        let allow = parent_status(refs[0].clone(), true);
        let mut other = allow.clone();
        other.parent_ref.namespace = Some("apps".into());
        other.parent_ref.group = Some(GATEWAY_API_GROUP.into());
        other.parent_ref.kind = Some("Gateway".into());
        for accepted in ["True", "False"] {
            other.conditions[0].status = accepted.into();
            for statuses in [vec![allow.clone(), other.clone()], vec![other.clone(), allow.clone()]] {
                assert!(filter_accepted_parent_refs(
                    Some(&refs),
                    Some(&statuses),
                    Some("apps"),
                    Some(&allow.controller_name)
                )
                .is_none());
            }
        }
        // A different port is a different attachment, not a duplicate rejection.
        other.parent_ref.port = Some(443);
        assert!(filter_accepted_parent_refs(
            Some(&refs),
            Some(&[allow.clone(), other]),
            Some("apps"),
            Some(&allow.controller_name)
        )
        .is_some());
    }

    #[test]
    fn delegation_only_parent_refs_are_excluded_with_or_without_status() {
        for kind in ["HTTPRoute", "GRPCRoute"] {
            let mut delegation = parent_ref("parent", None);
            delegation.group = Some(GATEWAY_API_GROUP.to_string());
            delegation.kind = Some(kind.to_string());
            let refs = vec![delegation.clone()];

            assert!(filter_accepted_parent_refs(
                Some(&refs),
                None,
                Some("route-ns"),
                Some("edgion.io/gateway-controller")
            )
            .is_none());

            let statuses = vec![parent_status(delegation, true)];
            assert!(filter_accepted_parent_refs(
                Some(&refs),
                Some(&statuses),
                Some("route-ns"),
                Some("edgion.io/gateway-controller")
            )
            .is_none());
        }
    }

    #[test]
    fn same_coordinate_delegation_status_cannot_override_gateway_status() {
        let gateway = parent_ref("shared-name", Some("shared-ns"));
        let mut delegation = gateway.clone();
        delegation.group = Some(GATEWAY_API_GROUP.to_string());
        delegation.kind = Some("HTTPRoute".to_string());
        let refs = vec![gateway.clone()];
        let statuses = vec![parent_status(delegation, true), parent_status(gateway, false)];

        assert!(filter_accepted_parent_refs(
            Some(&refs),
            Some(&statuses),
            Some("route-ns"),
            Some("edgion.io/gateway-controller")
        )
        .is_none());
    }

    #[test]
    fn omitted_and_explicit_gateway_defaults_match_the_same_status_identity() {
        let omitted = parent_ref("gw", Some("gw-ns"));
        let mut explicit = omitted.clone();
        explicit.group = Some(GATEWAY_API_GROUP.to_string());
        explicit.kind = Some("Gateway".to_string());
        let refs = vec![omitted, explicit.clone()];
        let statuses = vec![parent_status(explicit, true)];

        assert_eq!(
            filter_accepted_parent_refs(
                Some(&refs),
                Some(&statuses),
                Some("route-ns"),
                Some("edgion.io/gateway-controller")
            ),
            Some(refs)
        );
    }

    #[test]
    fn mixed_parent_refs_retain_only_the_accepted_gateways_in_order() {
        let first = parent_ref("first", None);
        let rejected = parent_ref("rejected", None);
        let mut delegation = parent_ref("child", None);
        delegation.kind = Some("GRPCRoute".to_string());
        let mut core_gateway = parent_ref("core-gateway", None);
        core_gateway.group = Some(String::new());
        core_gateway.kind = Some("Gateway".to_string());
        let last = parent_ref("last", Some("other-ns"));
        let refs = vec![
            first.clone(),
            delegation.clone(),
            rejected.clone(),
            core_gateway,
            last.clone(),
        ];
        let statuses = vec![
            parent_status(first.clone(), true),
            parent_status(delegation, true),
            parent_status(rejected, false),
            parent_status(last.clone(), true),
        ];

        assert_eq!(
            filter_accepted_parent_refs(
                Some(&refs),
                Some(&statuses),
                Some("route-ns"),
                Some("edgion.io/gateway-controller")
            ),
            Some(vec![first, last])
        );
    }

    #[test]
    fn resolve_parent_refs_uses_the_explicit_namespace_when_present() {
        let refs = vec![parent_ref("gw", Some("gw-ns"))];
        let resolved = resolve_parent_refs(&refs, "route-ns");

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].gateway_key, "gw-ns/gw");
        assert_eq!(resolved[0].parent_ns, "gw-ns");
    }

    #[test]
    fn resolve_parent_refs_falls_back_to_the_route_namespace() {
        // Gateway API: "When unspecified, this refers to the local namespace of
        // the Route." Omitting the namespace is the ordinary same-namespace
        // spelling, so this fallback is the common path, not an edge case.
        let refs = vec![parent_ref("gw", None)];
        let resolved = resolve_parent_refs(&refs, "route-ns");

        assert_eq!(resolved[0].gateway_key, "route-ns/gw");
        assert_eq!(resolved[0].parent_ns, "route-ns");
    }

    #[test]
    fn resolve_parent_refs_is_per_route_because_the_fallback_differs() {
        // The same ParentReference attached to routes in two namespaces must
        // resolve to two different keys. This is why resolution cannot be
        // cached per ParentReference and must stay per (route, parentRef).
        let refs = vec![parent_ref("gw", None)];

        let in_a = resolve_parent_refs(&refs, "ns-a");
        let in_b = resolve_parent_refs(&refs, "ns-b");

        assert_eq!(in_a[0].gateway_key, "ns-a/gw");
        assert_eq!(in_b[0].gateway_key, "ns-b/gw");
        assert_ne!(in_a[0].gateway_key, in_b[0].gateway_key);
    }

    #[test]
    fn resolve_parent_refs_keeps_parent_ns_recoverable_when_the_key_has_no_prefix() {
        // `format_gateway_key` emits a BARE NAME for an empty namespace, so
        // `gateway_key` is not always "<ns>/<name>". This is why `parent_ns` is
        // stored as its own field instead of being sliced back out of the key.
        // The Gateway API CRD forbids an explicit empty `namespace`
        // (`minLength: 1`) and both build sites supply a non-empty fallback, so
        // this shape is unreachable in production — the assertion locks the
        // layout decision, not a live code path.
        let refs = vec![parent_ref("gw", None)];
        let resolved = resolve_parent_refs(&refs, "");

        assert_eq!(resolved[0].gateway_key, "gw", "no namespace prefix to slice");
        assert_eq!(resolved[0].parent_ns, "");
    }

    #[test]
    fn resolve_parent_refs_preserves_order() {
        // `check_gateway_listener_match` returns the first match, so reordering
        // would change which Gateway a route attaches to.
        let refs = vec![
            parent_ref("first", None),
            parent_ref("second", Some("other-ns")),
            parent_ref("third", None),
        ];
        let resolved = resolve_parent_refs(&refs, "route-ns");

        let keys: Vec<&str> = resolved.iter().map(|r| r.gateway_key.as_str()).collect();
        assert_eq!(keys, vec!["route-ns/first", "other-ns/second", "route-ns/third"]);
    }

    #[test]
    fn resolve_parent_refs_does_not_filter_by_kind_or_group() {
        // The request path does not filter either — a non-Gateway ref simply
        // misses the index. `gateways_from_parent_refs` is the consumer that
        // filters; this one must stay behaviour-identical to the old inline
        // derivation, which saw every ref.
        let mut service_ref = parent_ref("some-service", None);
        service_ref.kind = Some("Service".to_string());
        service_ref.group = Some("example.io".to_string());
        let refs = vec![service_ref];

        let resolved = resolve_parent_refs(&refs, "route-ns");

        assert_eq!(resolved.len(), 1, "a non-Gateway parentRef must still be resolved");
        assert_eq!(resolved[0].gateway_key, "route-ns/some-service");

        // Contrast with the metric-handle consumer, which does filter.
        assert!(gateways_from_parent_refs(&refs, "route-ns").is_empty());
    }

    #[test]
    fn resolve_parent_refs_copies_the_section_name() {
        let mut with_section = parent_ref("gw", None);
        with_section.section_name = Some("https".to_string());
        let refs = vec![with_section, parent_ref("gw2", None)];

        let resolved = resolve_parent_refs(&refs, "route-ns");

        assert_eq!(resolved[0].section_name.as_deref(), Some("https"));
        assert_eq!(resolved[1].section_name, None);
    }

    #[test]
    fn resolve_parent_refs_preserves_declared_port_constraints() {
        let mut parent = parent_ref("gw", None);
        for port in [None, Some(8080), Some(8081)] {
            parent.port = port;
            let resolved = resolve_parent_refs(&[parent.clone()], "default");
            assert_eq!(resolved[0].port, port);
        }
    }

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

    #[test]
    fn l4_backend_ref_denied_literals_are_fixed_and_non_identifying() {
        assert_eq!(L4_BACKEND_REF_DENIED_STATUS, "RefDenied");
        assert_eq!(L4_BACKEND_REF_DENIED_LOG, "backend ref denied");
        assert_eq!(L4_BACKEND_UNSUPPORTED_KIND_STATUS, "UnsupportedBackend");
        assert_eq!(L4_BACKEND_UNSUPPORTED_KIND_LOG, "unsupported backend kind");
        assert!(
            !L4_BACKEND_REF_DENIED_LOG.contains("ReferenceGrant")
                && !L4_BACKEND_REF_DENIED_STATUS.contains("ReferenceGrant")
                && !L4_BACKEND_UNSUPPORTED_KIND_LOG.contains("Deployment")
        );
    }

    fn sample_denied() -> RefDenied {
        RefDenied {
            target_namespace: "other".to_string(),
            target_name: "svc".to_string(),
            reason: Some("NoMatchingReferenceGrant".to_string()),
        }
    }

    #[test]
    fn classify_l4_selected_backend_denied_is_terminal_and_wins_over_kind() {
        let denied = sample_denied();
        assert_eq!(
            classify_l4_selected_backend(Some(&denied), None, None),
            L4SelectedBackend::Denied
        );
        assert_eq!(
            classify_l4_selected_backend(Some(&denied), Some("apps"), Some("Deployment")),
            L4SelectedBackend::Denied
        );
        assert_ne!(L4SelectedBackend::Denied, L4SelectedBackend::Proceed);
        assert_ne!(L4SelectedBackend::UnsupportedKind, L4SelectedBackend::Proceed);
    }

    #[test]
    fn classify_l4_selected_backend_rejects_non_core_service_kind() {
        assert_eq!(
            classify_l4_selected_backend(None, Some("apps"), Some("Deployment")),
            L4SelectedBackend::UnsupportedKind
        );
        assert_eq!(
            classify_l4_selected_backend(None, Some("example.com"), Some("Service")),
            L4SelectedBackend::UnsupportedKind
        );
    }

    #[test]
    fn classify_l4_selected_backend_allows_core_service() {
        assert_eq!(
            classify_l4_selected_backend(None, None, None),
            L4SelectedBackend::Proceed
        );
        assert_eq!(
            classify_l4_selected_backend(None, Some(""), Some("Service")),
            L4SelectedBackend::Proceed
        );
        assert_eq!(
            classify_l4_selected_backend(None, Some("core"), Some("Service")),
            L4SelectedBackend::Proceed
        );
    }

    #[test]
    fn l4_post_select_denied_and_unsupported_are_fail_closed_not_retryable() {
        // Exhaustive: FailClosed | Proceed. A third Retry variant would
        // re-open the TLS continue-after-deny / continue-after-kind bug.
        let denied = sample_denied();
        let denied_action = l4_post_select(Some(&denied), None, None);
        assert_eq!(
            denied_action,
            L4PostSelect::FailClosed {
                status: L4_BACKEND_REF_DENIED_STATUS,
                log: L4_BACKEND_REF_DENIED_LOG,
            }
        );

        let kind_action = l4_post_select(None, Some("apps"), Some("Deployment"));
        assert_eq!(
            kind_action,
            L4PostSelect::FailClosed {
                status: L4_BACKEND_UNSUPPORTED_KIND_STATUS,
                log: L4_BACKEND_UNSUPPORTED_KIND_LOG,
            }
        );

        assert_eq!(l4_post_select(None, None, None), L4PostSelect::Proceed);
        assert!(!matches!(
            l4_post_select(None, None, None),
            L4PostSelect::FailClosed { .. }
        ));
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

    fn ka_annotations(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        let mut a = BTreeMap::new();
        for (k, v) in pairs {
            a.insert(k.to_string(), v.to_string());
        }
        a
    }

    #[test]
    fn resolve_tcp_keepalive_none_when_no_annotations() {
        assert_eq!(resolve_tcp_keepalive(None, "ns/r"), None);
    }

    #[test]
    fn resolve_tcp_keepalive_none_when_time_absent() {
        let a = ka_annotations(&[("edgion.io/tcp-keepalive-interval", "10")]);
        assert_eq!(resolve_tcp_keepalive(Some(&a), "ns/r"), None);
    }

    #[test]
    fn resolve_tcp_keepalive_none_when_time_nonpositive_or_invalid() {
        let zero = ka_annotations(&[("edgion.io/tcp-keepalive-time", "0")]);
        assert_eq!(resolve_tcp_keepalive(Some(&zero), "ns/r"), None);
        let neg = ka_annotations(&[("edgion.io/tcp-keepalive-time", "-5")]);
        assert_eq!(resolve_tcp_keepalive(Some(&neg), "ns/r"), None);
        let nan = ka_annotations(&[("edgion.io/tcp-keepalive-time", "abc")]);
        assert_eq!(resolve_tcp_keepalive(Some(&nan), "ns/r"), None);
    }

    #[test]
    fn resolve_tcp_keepalive_defaults_when_only_time_set() {
        let a = ka_annotations(&[("edgion.io/tcp-keepalive-time", "300")]);
        let got = resolve_tcp_keepalive(Some(&a), "ns/r").expect("enabled");
        assert_eq!(got.idle, std::time::Duration::from_secs(300));
        assert_eq!(got.interval, std::time::Duration::from_secs(75));
        assert_eq!(got.count, 9);
    }

    #[test]
    fn resolve_tcp_keepalive_all_fields_in_range() {
        let a = ka_annotations(&[
            ("edgion.io/tcp-keepalive-time", "60"),
            ("edgion.io/tcp-keepalive-interval", "10"),
            ("edgion.io/tcp-keepalive-probes", "4"),
        ]);
        let got = resolve_tcp_keepalive(Some(&a), "ns/r").expect("enabled");
        assert_eq!(got.idle, std::time::Duration::from_secs(60));
        assert_eq!(got.interval, std::time::Duration::from_secs(10));
        assert_eq!(got.count, 4);
    }

    #[test]
    fn resolve_tcp_keepalive_clamps_out_of_range() {
        let a = ka_annotations(&[
            ("edgion.io/tcp-keepalive-time", "999999"),
            ("edgion.io/tcp-keepalive-interval", "9999"),
            ("edgion.io/tcp-keepalive-probes", "9999"),
        ]);
        let got = resolve_tcp_keepalive(Some(&a), "ns/r").expect("enabled");
        assert_eq!(got.idle, std::time::Duration::from_secs(7200));
        assert_eq!(got.interval, std::time::Duration::from_secs(75));
        assert_eq!(got.count, 20);
    }

    #[test]
    fn to_pingora_maps_fields() {
        let cfg = TcpKeepaliveConfig {
            idle: std::time::Duration::from_secs(120),
            interval: std::time::Duration::from_secs(30),
            count: 5,
        };
        let ka = to_pingora_tcp_keepalive(&cfg);
        assert_eq!(ka.idle, std::time::Duration::from_secs(120));
        assert_eq!(ka.interval, std::time::Duration::from_secs(30));
        assert_eq!(ka.count, 5);
        #[cfg(target_os = "linux")]
        assert_eq!(ka.user_timeout, std::time::Duration::ZERO);
    }

    #[test]
    fn resolve_tcp_keepalive_invalid_interval_and_probes_fall_back_to_defaults() {
        let a = ka_annotations(&[
            ("edgion.io/tcp-keepalive-time", "300"),
            ("edgion.io/tcp-keepalive-interval", "0"),
            ("edgion.io/tcp-keepalive-probes", "abc"),
        ]);
        let got = resolve_tcp_keepalive(Some(&a), "ns/r").expect("enabled");
        assert_eq!(got.idle, std::time::Duration::from_secs(300));
        assert_eq!(got.interval, std::time::Duration::from_secs(75));
        assert_eq!(got.count, 9);
    }

    #[test]
    fn resolve_tcp_keepalive_exact_boundaries() {
        // time: 7200 kept, 7201 clamped to 7200
        let a = ka_annotations(&[("edgion.io/tcp-keepalive-time", "7200")]);
        assert_eq!(
            resolve_tcp_keepalive(Some(&a), "r").unwrap().idle,
            std::time::Duration::from_secs(7200)
        );
        let a = ka_annotations(&[("edgion.io/tcp-keepalive-time", "7201")]);
        assert_eq!(
            resolve_tcp_keepalive(Some(&a), "r").unwrap().idle,
            std::time::Duration::from_secs(7200)
        );
        // interval: 75 kept, 76 clamped
        let a = ka_annotations(&[
            ("edgion.io/tcp-keepalive-time", "300"),
            ("edgion.io/tcp-keepalive-interval", "75"),
        ]);
        assert_eq!(
            resolve_tcp_keepalive(Some(&a), "r").unwrap().interval,
            std::time::Duration::from_secs(75)
        );
        let a = ka_annotations(&[
            ("edgion.io/tcp-keepalive-time", "300"),
            ("edgion.io/tcp-keepalive-interval", "76"),
        ]);
        assert_eq!(
            resolve_tcp_keepalive(Some(&a), "r").unwrap().interval,
            std::time::Duration::from_secs(75)
        );
        // probes: 20 kept, 21 clamped
        let a = ka_annotations(&[
            ("edgion.io/tcp-keepalive-time", "300"),
            ("edgion.io/tcp-keepalive-probes", "20"),
        ]);
        assert_eq!(resolve_tcp_keepalive(Some(&a), "r").unwrap().count, 20);
        let a = ka_annotations(&[
            ("edgion.io/tcp-keepalive-time", "300"),
            ("edgion.io/tcp-keepalive-probes", "21"),
        ]);
        assert_eq!(resolve_tcp_keepalive(Some(&a), "r").unwrap().count, 20);
    }

    #[test]
    fn resolve_tcp_keepalive_clamps_interval_independently() {
        let a = ka_annotations(&[
            ("edgion.io/tcp-keepalive-time", "300"),
            ("edgion.io/tcp-keepalive-interval", "9999"),
            ("edgion.io/tcp-keepalive-probes", "5"),
        ]);
        let got = resolve_tcp_keepalive(Some(&a), "r").unwrap();
        assert_eq!(got.idle, std::time::Duration::from_secs(300));
        assert_eq!(got.interval, std::time::Duration::from_secs(75));
        assert_eq!(got.count, 5);
    }

    #[test]
    fn resolve_tcp_keepalive_clamps_probes_independently() {
        let a = ka_annotations(&[
            ("edgion.io/tcp-keepalive-time", "300"),
            ("edgion.io/tcp-keepalive-interval", "30"),
            ("edgion.io/tcp-keepalive-probes", "9999"),
        ]);
        let got = resolve_tcp_keepalive(Some(&a), "r").unwrap();
        assert_eq!(got.interval, std::time::Duration::from_secs(30));
        assert_eq!(got.count, 20);
    }

    #[test]
    fn resolve_tcp_keepalive_partial_field_defaults() {
        // interval explicit, probes absent -> probes defaults to 9
        let a = ka_annotations(&[
            ("edgion.io/tcp-keepalive-time", "60"),
            ("edgion.io/tcp-keepalive-interval", "15"),
        ]);
        let got = resolve_tcp_keepalive(Some(&a), "r").unwrap();
        assert_eq!(got.interval, std::time::Duration::from_secs(15));
        assert_eq!(got.count, 9);
        // probes explicit, interval absent -> interval defaults to 75
        let a = ka_annotations(&[
            ("edgion.io/tcp-keepalive-time", "60"),
            ("edgion.io/tcp-keepalive-probes", "5"),
        ]);
        let got = resolve_tcp_keepalive(Some(&a), "r").unwrap();
        assert_eq!(got.interval, std::time::Duration::from_secs(75));
        assert_eq!(got.count, 5);
    }
}
