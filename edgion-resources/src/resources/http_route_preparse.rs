//! Hidden logic structures for internal processing
//! These structures are not serialized, only used for runtime analysis

use super::{HTTPRoute, HTTPRouteFilterType, LocalObjectReference};
use crate::path_normalization::normalize_config_path;
use serde::{Deserialize, Serialize};

/// Hash source type for consistent hash LB policy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsistentHashOn {
    /// Hash based on request header
    Header(String),
    /// Hash based on cookie
    Cookie(String),
    /// Hash based on query argument
    Arg(String),
}

/// Parsed LB policy from extensionRef
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParsedLBPolicy {
    /// Consistent hash with specified source
    ConsistentHash(ConsistentHashOn),
    /// Least connection
    LeastConn,
    /// EWMA (Exponentially Weighted Moving Average) based on response time
    Ewma,
}

/// Parsed extension info attached to HTTPBackendRef
/// This is computed at runtime, not from YAML
#[derive(Debug, Clone, Default)]
pub struct BackendExtensionInfo {
    /// Parsed LB policy if extensionRef specifies one
    pub lb_policy: Option<ParsedLBPolicy>,
}

impl BackendExtensionInfo {
    /// Parse from LocalObjectReference
    pub fn from_extension_ref(ext_ref: &LocalObjectReference) -> Self {
        let lb_policy = Self::parse_lb_policy(ext_ref);
        Self { lb_policy }
    }

    /// Parse LB policy from extensionRef
    ///
    /// Supported formats:
    /// - group: edgion.io, kind: LBPolicyConsistentHash, name: header.x-user-id
    /// - group: edgion.io, kind: LBPolicyConsistentHash, name: cookie.session-id
    /// - group: edgion.io, kind: LBPolicyConsistentHash, name: arg.user-id
    /// - group: edgion.io, kind: LBPolicyLeastConn, name: default
    /// - group: edgion.io, kind: LBPolicyEwma, name: default
    fn parse_lb_policy(ext_ref: &LocalObjectReference) -> Option<ParsedLBPolicy> {
        // Check group (empty string means core API group, otherwise should be edgion.io)
        if !ext_ref.group.is_empty() && ext_ref.group != "edgion.io" {
            return None;
        }

        // `is_lb_policy_kind` is the single source of truth for which kinds are
        // LBPolicy kinds. Bail out early for any non-LBPolicy kind so the kind
        // list never needs to be duplicated here.
        if !is_lb_policy_kind(&ext_ref.kind) {
            return None;
        }

        match ext_ref.kind.as_str() {
            "LBPolicyConsistentHash" => {
                Self::parse_consistent_hash_name(&ext_ref.name).map(ParsedLBPolicy::ConsistentHash)
            }
            "LBPolicyLeastConn" => Some(ParsedLBPolicy::LeastConn),
            "LBPolicyEwma" => Some(ParsedLBPolicy::Ewma),
            _ => None,
        }
    }

    /// Parse consistent hash source from name
    /// Format: "header.xxx" / "cookie.xxx" / "arg.xxx"
    fn parse_consistent_hash_name(name: &str) -> Option<ConsistentHashOn> {
        let parts: Vec<&str> = name.splitn(2, '.').collect();
        match parts.as_slice() {
            ["header", key] => Some(ConsistentHashOn::Header(key.to_string())),
            ["cookie", key] => Some(ConsistentHashOn::Cookie(key.to_string())),
            ["arg", key] => Some(ConsistentHashOn::Arg(key.to_string())),
            _ => None,
        }
    }
}

/// True if `kind` names a load-balancing-policy ExtensionRef (as opposed to a
/// plugin reference). LBPolicy ExtensionRefs live on `backendRefs[].filters` and
/// are consumed by `BackendExtensionInfo` / `parse_lb_policy`, not the plugin
/// runtime — callers that gate "is this a supported ExtensionRef kind" must
/// treat these as recognized, not unsupported.
pub fn is_lb_policy_kind(kind: &str) -> bool {
    matches!(kind, "LBPolicyConsistentHash" | "LBPolicyLeastConn" | "LBPolicyEwma")
}

/// Extension trait for HTTPRoute to parse hidden logic
impl HTTPRoute {
    /// Parse all extension_ref in backend_refs and populate extension_info fields.
    ///
    /// This method should be called after deserializing HTTPRoute from YAML/JSON
    /// to populate the runtime-only extension_info fields.
    ///
    /// # Incomplete by itself
    ///
    /// `preparse()` only covers plugin runtimes, path-matcher normalization, and
    /// the `forward-raw-path` annotation. It does **not** parse timeouts or retry
    /// configurations. A fully pre-processed `HTTPRoute` requires all three calls:
    ///
    /// ```ignore
    /// route.preparse();
    /// route.parse_timeouts();
    /// route.parse_retry();
    /// ```
    ///
    /// Production code paths go through `ResourceMeta::pre_parse()` (implemented
    /// via `impl_resource_meta!` in `types/resource/meta/impls.rs`), which chains
    /// all three. Prefer `pre_parse()` over calling `preparse()` directly unless
    /// you intentionally want only the subset above.
    pub fn preparse(&mut self) {
        // Normalize path matcher values before any other processing so downstream
        // logic always sees the canonical path representation.
        self.normalize_path_matchers();

        let Some(rules) = self.spec.rules.as_mut() else {
            return;
        };

        // Parse `edgion.io/forward-raw-path` annotation. Three-state:
        // "true" / "false" / absent. Invalid values logged INFO and treated as None.
        let parsed_forward_raw_path: Option<bool> = self
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get(crate::constants::annotations::edgion::FORWARD_RAW_PATH))
            .and_then(|v| match v.as_str() {
                "true" => Some(true),
                "false" => Some(false),
                other => {
                    tracing::info!(
                        route_namespace = %self.metadata.namespace.as_deref().unwrap_or("default"),
                        route_name = %self.metadata.name.as_deref().unwrap_or(""),
                        annotation = %crate::constants::annotations::edgion::FORWARD_RAW_PATH,
                        value = %other,
                        "ignored: expected \"true\" or \"false\""
                    );
                    None
                }
            });

        for rule in rules.iter_mut() {
            rule.parsed_forward_raw_path = parsed_forward_raw_path;

            let Some(backend_refs) = rule.backend_refs.as_mut() else {
                continue;
            };

            for backend_ref in backend_refs.iter_mut() {
                // Find ExtensionRef filter in backend_ref.plugins
                let extension_info = backend_ref
                    .filters
                    .as_ref()
                    .and_then(|filters| {
                        filters
                            .iter()
                            .find(|f| f.filter_type == HTTPRouteFilterType::ExtensionRef)
                            .and_then(|f| f.extension_ref.as_ref())
                            .map(BackendExtensionInfo::from_extension_ref)
                    })
                    .unwrap_or_default();

                backend_ref.extension_info = extension_info;
            }
        }
    }

    /// Parse and pre-process timeout configurations for all rules
    ///
    /// This method is called during route loading (in pre_parse) to avoid runtime parsing overhead.
    /// It parses timeout strings into Duration objects and stores them in rule.parsed_timeouts.
    pub fn parse_timeouts(&mut self) {
        let Some(rules) = self.spec.rules.as_mut() else {
            return;
        };

        for rule in rules.iter_mut() {
            // Parse timeouts for each rule
            if let Some(timeouts) = &rule.timeouts {
                rule.parsed_timeouts = crate::resources::http_route::ParsedRouteTimeouts::from_config(timeouts);

                if rule.parsed_timeouts.is_some() {
                    tracing::debug!("Parsed route-level timeouts for HTTPRoute rule");
                }
            }
        }
    }

    /// Normalize path matcher values for all rules at load time.
    ///
    /// For `Exact` and `PathPrefix` match types, applies [`normalize_config_path`] so the
    /// stored value inhabits the same normalized space as the request-side path produced by
    /// Group E (which reads `ctx.path()`, the already-normalized current path).
    ///
    /// `RegularExpression` patterns are left unchanged — regex patterns cannot be
    /// machine-normalized; correctness is the operator's responsibility.
    pub fn normalize_path_matchers(&mut self) {
        let route_ns = self.metadata.namespace.as_deref().unwrap_or("default");
        let route_name = self.metadata.name.as_deref().unwrap_or("");

        let Some(rules) = self.spec.rules.as_mut() else {
            return;
        };

        let path_cfg = crate::path_normalization::get_path_normalization_config();
        for rule in rules.iter_mut() {
            let Some(matches) = rule.matches.as_mut() else {
                continue;
            };
            for m in matches.iter_mut() {
                let Some(path) = m.path.as_mut() else {
                    continue;
                };
                // Only normalize Exact and PathPrefix; skip RegularExpression and unknown types.
                let match_type = path.match_type.as_deref().unwrap_or("PathPrefix");
                if match_type == "Exact" || match_type == "PathPrefix" {
                    if let Some(value) = path.value.as_mut() {
                        let normalized = normalize_config_path(value, path_cfg);
                        if normalized != *value {
                            tracing::info!(
                                route = %format!("{}/{}", route_ns, route_name),
                                match_type = %match_type,
                                original = %value,
                                normalized = %normalized,
                                "HTTPRoute path matcher value normalized at load time"
                            );
                            *value = normalized;
                        }
                    }
                }
            }
        }
    }

    /// Parse and pre-process retry configurations for all rules
    pub fn parse_retry(&mut self) {
        let Some(rules) = self.spec.rules.as_mut() else {
            return;
        };

        for rule in rules.iter_mut() {
            let Some(retry) = &rule.retry else {
                continue;
            };

            let attempts = retry.attempts.map(|a| a.max(0) as u32);

            let backoff = retry.backoff.as_deref().and_then(|s| {
                crate::duration::parse_duration(s)
                    .map_err(|e| {
                        tracing::warn!(
                            route = %format!("{}/{}",
                                self.metadata.namespace.as_deref().unwrap_or("default"),
                                self.metadata.name.as_deref().unwrap_or("")),
                            value = %s,
                            error = %e,
                            "Invalid retry backoff duration, backoff disabled"
                        );
                    })
                    .ok()
            });

            let codes = retry.codes.as_ref().map(|codes| {
                codes
                    .iter()
                    .filter_map(|&c| {
                        // Retry status codes are restricted to 400..=599 to match
                        // the upstream Gateway API HTTPRouteRetry CRD validation.
                        // Informational (1xx) and success/redirect (2xx/3xx) codes
                        // are never a meaningful retry trigger and a 1xx code here
                        // would otherwise let retry-on-status misfire on an h1
                        // informational header.
                        if (400..=599).contains(&c) {
                            Some(c as u16)
                        } else {
                            tracing::warn!(code = c, "Invalid HTTP status code in retry.codes, ignoring");
                            None
                        }
                    })
                    .collect::<Vec<u16>>()
            });

            if attempts.is_some() || backoff.is_some() || codes.is_some() {
                rule.parsed_retry = Some(crate::resources::http_route::ParsedRouteRetry {
                    attempts,
                    backoff,
                    codes,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_consistent_hash_header() {
        let ext_ref = LocalObjectReference {
            group: "edgion.io".to_string(),
            kind: "LBPolicyConsistentHash".to_string(),
            name: "header.x-user-id".to_string(),
        };

        let info = BackendExtensionInfo::from_extension_ref(&ext_ref);
        assert_eq!(
            info.lb_policy,
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Header(
                "x-user-id".to_string()
            )))
        );
    }

    #[test]
    fn test_parse_consistent_hash_cookie() {
        let ext_ref = LocalObjectReference {
            group: "edgion.io".to_string(),
            kind: "LBPolicyConsistentHash".to_string(),
            name: "cookie.session-id".to_string(),
        };

        let info = BackendExtensionInfo::from_extension_ref(&ext_ref);
        assert_eq!(
            info.lb_policy,
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Cookie(
                "session-id".to_string()
            )))
        );
    }

    #[test]
    fn test_parse_consistent_hash_arg() {
        let ext_ref = LocalObjectReference {
            group: "edgion.io".to_string(),
            kind: "LBPolicyConsistentHash".to_string(),
            name: "arg.user-id".to_string(),
        };

        let info = BackendExtensionInfo::from_extension_ref(&ext_ref);
        assert_eq!(
            info.lb_policy,
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Arg(
                "user-id".to_string()
            )))
        );
    }

    #[test]
    fn test_parse_least_conn() {
        let ext_ref = LocalObjectReference {
            group: "edgion.io".to_string(),
            kind: "LBPolicyLeastConn".to_string(),
            name: "default".to_string(),
        };

        let info = BackendExtensionInfo::from_extension_ref(&ext_ref);
        assert_eq!(info.lb_policy, Some(ParsedLBPolicy::LeastConn));
    }

    #[test]
    fn test_parse_unknown_kind() {
        let ext_ref = LocalObjectReference {
            group: "edgion.io".to_string(),
            kind: "UnknownPolicy".to_string(),
            name: "default".to_string(),
        };

        let info = BackendExtensionInfo::from_extension_ref(&ext_ref);
        assert_eq!(info.lb_policy, None);
    }

    #[test]
    fn test_parse_wrong_group() {
        let ext_ref = LocalObjectReference {
            group: "other.io".to_string(),
            kind: "LBPolicyConsistentHash".to_string(),
            name: "header.x-user-id".to_string(),
        };

        let info = BackendExtensionInfo::from_extension_ref(&ext_ref);
        assert_eq!(info.lb_policy, None);
    }

    #[test]
    fn test_parse_empty_group() {
        // Empty group (core API group), should still work
        let ext_ref = LocalObjectReference {
            group: String::new(),
            kind: "LBPolicyConsistentHash".to_string(),
            name: "header.x-user-id".to_string(),
        };

        let info = BackendExtensionInfo::from_extension_ref(&ext_ref);
        assert_eq!(
            info.lb_policy,
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Header(
                "x-user-id".to_string()
            )))
        );
    }

    // =========================================================================
    // Group J: HTTPRoute path matcher normalization tests
    // =========================================================================

    fn make_route_with_path(match_type: &str, value: &str) -> HTTPRoute {
        serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {"name": "test", "namespace": "default"},
            "spec": {
                "rules": [{
                    "matches": [{
                        "path": {"type": match_type, "value": value}
                    }]
                }]
            }
        }))
        .expect("route should deserialize")
    }

    fn get_path_value(route: &HTTPRoute) -> &str {
        route.spec.rules.as_ref().unwrap()[0].matches.as_ref().unwrap()[0]
            .path
            .as_ref()
            .unwrap()
            .value
            .as_deref()
            .unwrap()
    }

    #[test]
    fn http_route_path_match_exact_normalized() {
        // Trailing slash should be removed by dot-segment/normalization logic.
        // normalize_config_path("/admin/") should produce "/admin/" — actually trailing
        // slash is not removed by the BASE algorithm; test with a genuine normalizable input.
        // Input with percent-encoded slash: "/api%2Fv1" → "/api/v1"
        let mut route = make_route_with_path("Exact", "/admin/");
        route.normalize_path_matchers();
        // "/admin/" has no dot-segments or percent-encoding; normalize is a no-op → unchanged
        assert_eq!(get_path_value(&route), "/admin/");

        // A value that genuinely needs normalization (double slash collapsed)
        let mut route2 = make_route_with_path("Exact", "/admin//foo");
        route2.normalize_path_matchers();
        assert_eq!(get_path_value(&route2), "/admin/foo");
    }

    #[test]
    fn http_route_path_match_exact_percent_encoded_normalized() {
        // Percent-encoded slash should be decoded: "/api%2Fv1" → "/api/v1"
        let mut route = make_route_with_path("Exact", "/api%2Fv1");
        route.normalize_path_matchers();
        assert_eq!(get_path_value(&route), "/api/v1");
    }

    #[test]
    fn http_route_path_match_pathprefix_normalized() {
        // Percent-encoded slash decoded: "/api%2Fv1" → "/api/v1"
        let mut route = make_route_with_path("PathPrefix", "/api%2Fv1");
        route.normalize_path_matchers();
        assert_eq!(get_path_value(&route), "/api/v1");
    }

    #[test]
    fn http_route_path_match_regex_not_normalized() {
        // RegularExpression values must not be modified
        let input = "/admin%2F.*";
        let mut route = make_route_with_path("RegularExpression", input);
        route.normalize_path_matchers();
        assert_eq!(get_path_value(&route), input);
    }

    #[test]
    fn http_route_path_match_already_normalized_unchanged() {
        // Already-normalized value — normalize_config_path is a no-op
        let mut route = make_route_with_path("Exact", "/api/v1");
        route.normalize_path_matchers();
        assert_eq!(get_path_value(&route), "/api/v1");
    }

    #[test]
    fn http_route_path_match_dot_segment_normalized() {
        // Dot segments should be resolved: "/api/v1/./foo" → "/api/v1/foo"
        let mut route = make_route_with_path("PathPrefix", "/api/v1/./foo");
        route.normalize_path_matchers();
        assert_eq!(get_path_value(&route), "/api/v1/foo");
    }

    // =========================================================================
    // forward-raw-path annotation parsing tests
    // =========================================================================

    fn make_test_http_route() -> HTTPRoute {
        serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {"name": "test-route", "namespace": "default"},
            "spec": {
                "rules": [
                    {
                        "matches": [{"path": {"type": "PathPrefix", "value": "/api"}}]
                    }
                ]
            }
        }))
        .expect("test HTTPRoute should deserialize")
    }

    #[test]
    fn parses_forward_raw_path_true() {
        let mut route = make_test_http_route();
        let mut anns = std::collections::BTreeMap::new();
        anns.insert(
            crate::constants::annotations::edgion::FORWARD_RAW_PATH.to_string(),
            "true".to_string(),
        );
        route.metadata.annotations = Some(anns);
        route.preparse();
        for rule in route.spec.rules.as_ref().unwrap() {
            assert_eq!(rule.parsed_forward_raw_path, Some(true));
        }
    }

    #[test]
    fn parses_forward_raw_path_false() {
        let mut route = make_test_http_route();
        let mut anns = std::collections::BTreeMap::new();
        anns.insert(
            crate::constants::annotations::edgion::FORWARD_RAW_PATH.to_string(),
            "false".to_string(),
        );
        route.metadata.annotations = Some(anns);
        route.preparse();
        for rule in route.spec.rules.as_ref().unwrap() {
            assert_eq!(rule.parsed_forward_raw_path, Some(false));
        }
    }

    #[test]
    fn parses_forward_raw_path_absent_is_none() {
        let mut route = make_test_http_route();
        route.metadata.annotations = None;
        route.preparse();
        for rule in route.spec.rules.as_ref().unwrap() {
            assert_eq!(rule.parsed_forward_raw_path, None);
        }
    }

    #[test]
    fn parses_forward_raw_path_invalid_is_none() {
        let mut route = make_test_http_route();
        let mut anns = std::collections::BTreeMap::new();
        anns.insert(
            crate::constants::annotations::edgion::FORWARD_RAW_PATH.to_string(),
            "yes".to_string(),
        );
        route.metadata.annotations = Some(anns);
        route.preparse();
        for rule in route.spec.rules.as_ref().unwrap() {
            assert_eq!(rule.parsed_forward_raw_path, None);
        }
    }

    #[test]
    fn parses_forward_raw_path_uppercase_is_invalid() {
        // Strict match: only lowercase "true" / "false" accepted.
        // "True" / "TRUE" / "False" all log INFO and treat as None.
        let mut route = make_test_http_route();
        let mut anns = std::collections::BTreeMap::new();
        anns.insert(
            crate::constants::annotations::edgion::FORWARD_RAW_PATH.to_string(),
            "True".to_string(),
        );
        route.metadata.annotations = Some(anns);
        route.preparse();
        for rule in route.spec.rules.as_ref().unwrap() {
            assert_eq!(rule.parsed_forward_raw_path, None);
        }
    }

    #[test]
    fn parses_forward_raw_path_with_whitespace_is_invalid() {
        // Whitespace not trimmed: "  true  " is invalid.
        let mut route = make_test_http_route();
        let mut anns = std::collections::BTreeMap::new();
        anns.insert(
            crate::constants::annotations::edgion::FORWARD_RAW_PATH.to_string(),
            "  true  ".to_string(),
        );
        route.metadata.annotations = Some(anns);
        route.preparse();
        for rule in route.spec.rules.as_ref().unwrap() {
            assert_eq!(rule.parsed_forward_raw_path, None);
        }
    }

    #[test]
    fn parses_forward_raw_path_empty_is_invalid() {
        // Empty value: "" is invalid (operator likely meant to delete the annotation).
        let mut route = make_test_http_route();
        let mut anns = std::collections::BTreeMap::new();
        anns.insert(
            crate::constants::annotations::edgion::FORWARD_RAW_PATH.to_string(),
            String::new(),
        );
        route.metadata.annotations = Some(anns);
        route.preparse();
        for rule in route.spec.rules.as_ref().unwrap() {
            assert_eq!(rule.parsed_forward_raw_path, None);
        }
    }

    // =========================================================================
    // retry.codes range validation (400..=599, matching the Gateway API CRD)
    // =========================================================================

    fn make_route_with_retry_codes(codes: Vec<i32>) -> HTTPRoute {
        serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {"name": "test-route", "namespace": "default"},
            "spec": {
                "rules": [{
                    "matches": [{"path": {"type": "PathPrefix", "value": "/api"}}],
                    "retry": {"codes": codes}
                }]
            }
        }))
        .expect("retry route should deserialize")
    }

    #[test]
    fn parse_retry_codes_clamped_to_400_599() {
        // 1xx/2xx/3xx codes are outside the Gateway API HTTPRouteRetry range and
        // must be dropped; only 400..=599 survive preparse.
        let mut route = make_route_with_retry_codes(vec![100, 103, 204, 302, 400, 503, 599, 600]);
        route.parse_retry();
        let parsed = route.spec.rules.as_ref().unwrap()[0]
            .parsed_retry
            .as_ref()
            .expect("parsed_retry should be set");
        assert_eq!(parsed.codes.as_deref(), Some([400u16, 503, 599].as_slice()));
    }

    #[test]
    fn parse_retry_codes_all_out_of_range_yields_empty() {
        let mut route = make_route_with_retry_codes(vec![100, 200, 399]);
        route.parse_retry();
        let parsed = route.spec.rules.as_ref().unwrap()[0]
            .parsed_retry
            .as_ref()
            .expect("parsed_retry should be set");
        assert_eq!(parsed.codes.as_deref(), Some([].as_slice()));
    }
}
