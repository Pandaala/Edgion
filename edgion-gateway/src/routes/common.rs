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
/// The current projection must contain exactly one accepted owned entry.
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
        return false;
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
/// A nonempty Controller-owned transport stamp and current status are required.
/// Missing owned status cannot authorize an attachment; foreign status is ignored.
pub(crate) fn filter_accepted_parent_refs(
    parent_refs: Option<&[ParentReference]>,
    status_parents: Option<&[RouteParentStatus]>,
    route_ns: Option<&str>,
    expected_controller: Option<&str>,
) -> Option<Vec<ParentReference>> {
    let expected_controller = expected_controller.filter(|owner| !owner.is_empty())?;
    let parent_refs = parent_refs?;
    let status_parents = status_parents?;
    if parent_refs.is_empty() {
        return None;
    }

    let accepted: Vec<ParentReference> = parent_refs
        .iter()
        .filter(|pr| {
            is_gateway_parent_ref(pr) && is_parent_ref_accepted(pr, status_parents, route_ns, expected_controller)
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
/// group is `""` / omitted and kind is `"Service"` / omitted.
///
/// Single source of truth shared by all L4 route modules (TCP, TLS termination,
/// TLS passthrough, UDP). Struct-based callers pass
/// `br.group.as_deref(), br.kind.as_deref()`.
pub(crate) fn is_core_service_backend_ref(group: Option<&str>, kind: Option<&str>) -> bool {
    edgion_resources::resources::refs::targets::is_standard_service(group, kind)
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
#[path = "../../tests/unit/routes/common_test.rs"]
mod tests;
