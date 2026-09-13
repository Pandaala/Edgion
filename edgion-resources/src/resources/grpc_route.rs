//! GRPCRoute resource definition
//!
//! GRPCRoute defines gRPC rules for mapping requests to backends

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

use super::common::{ParentReference, RefDenied};
use super::http_route::{
    DelegationIssue, HTTPHeader, LocalObjectReference, ParsedRouteRetry,
    ParsedRouteTimeouts as HttpParsedRouteTimeouts, RouteTimeout as HttpRouteTimeout, SessionPersistence,
    GATEWAY_API_GROUP,
};
use crate::ListenerKey;

/// GRPCRoute defines gRPC rules for mapping requests to backends
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "GRPCRoute",
    plural = "grpcroutes",
    status = "GRPCRouteStatus",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct GRPCRouteSpec {
    /// ParentRefs references the resources that this Route wants to be attached to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<ParentReference>>,

    /// Hostnames defines a set of hostnames to match against the gRPC Host header
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(inner(
        length(max = crate::gwapi_types::HOSTNAME_MAX_LEN),
        pattern(crate::gwapi_types::HOSTNAME_PATTERN)
    ))]
    pub hostnames: Option<Vec<String>>,

    /// Controller-resolved effective hostnames (intersection of route hostnames and listener hostnames).
    /// Eligibility only; hostname precedence uses the route's authored hostnames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_hostnames: Option<Vec<String>>,

    /// Pre-computed listener keys from parentRef resolution (set by Controller).
    /// Used by Gateway to bind this route to per-listener route tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_listeners: Option<Vec<ListenerKey>>,

    /// Controller-identified invalid rule indices.
    /// Gateway skips these rules during route compilation.
    /// Indices refer to `rules`; not applicable to `resolved_rules`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub invalid_rule_indices: Option<Vec<usize>>,

    /// Rules defines the gRPC routing rules
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<GRPCRouteRule>>,

    /// Controller-resolved flattened rules produced by GRPCRoute delegation
    /// (a rule whose backendRef points at `kind: GRPCRoute` delegates its
    /// method/service match to child routes; the controller expands the
    /// delegation tree). Present only on routes containing at least one
    /// delegating rule. When present, the gateway compiles these INSTEAD of
    /// `rules`, and `invalid_rule_indices` does not apply (flattening already
    /// dropped invalid rules). See docs/en/user-guide/grpc-route/delegation.md
    /// and skills/01-architecture/05-resources/05-grpc-route.md.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_rules: Option<Vec<GRPCRouteRule>>,

    /// Controller-internal delegation diagnostics, produced by the flatten
    /// step in `parse` and consumed by `update_status`. Never serialized.
    #[serde(skip, default)]
    #[schemars(skip)]
    pub delegation_issues: Vec<DelegationIssue>,
}

impl GRPCRouteSpec {
    /// Validate authored hostname syntax and the Gateway API list bound.
    pub fn validate_hostnames(&self) -> Result<(), crate::HostnameError> {
        crate::validate_route_hostnames(self.hostnames.as_deref(), false)
    }
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCRouteRule {
    /// Name is the name of the rule. It can be used for observability and status reporting.
    /// Support: Extended
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Matches define conditions used for matching the rule against requests
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches: Option<Vec<GRPCRouteMatch>>,

    /// Filters define the plugins that are applied to requests
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Vec<GRPCRouteFilter>>,

    /// BackendRefs defines the backend(s) where matching requests should be sent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_refs: Option<Vec<GRPCBackendRef>>,

    /// Timeouts defines timeouts for requests matching this rule
    /// Support: Extended
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeouts: Option<GRPCRouteTimeouts>,

    /// Retry defines the retry policy for requests matching this rule
    /// Support: Extended (Experimental)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<GRPCRouteRetry>,

    /// SessionPersistence defines the session persistence configuration
    /// Support: Extended (Experimental)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_persistence: Option<SessionPersistence>,

    /// Pre-parsed route-level timeout configurations (not serialized)
    /// Parsed once when route is loaded, used at runtime
    #[serde(skip)]
    #[schemars(skip)]
    pub parsed_timeouts: Option<HttpParsedRouteTimeouts>,

    /// Pre-parsed retry configuration from GRPCRouteRule.retry (not serialized)
    /// Only attempts and backoff are supported; codes require gRPC trailers interception (not yet implemented)
    #[serde(skip)]
    #[schemars(skip)]
    pub parsed_retry: Option<ParsedRouteRetry>,

    /// Pre-parsed `edgion.io/retry-non-idempotent` annotation from parent
    /// GRPCRoute.metadata. gRPC is always POST at the HTTP layer, so by default
    /// it is treated as non-idempotent and not retried on a reused-connection
    /// reset; `Some(true)` opts back into that retry. Parsed once at load time
    /// and copied to every rule under that route.
    #[serde(skip)]
    #[schemars(skip)]
    pub parsed_allow_non_idempotent_retry: Option<bool>,
}

impl Clone for GRPCRouteRule {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            matches: self.matches.clone(),
            filters: self.filters.clone(),
            backend_refs: self.backend_refs.clone(),
            timeouts: self.timeouts.clone(),
            retry: self.retry.clone(),
            session_persistence: self.session_persistence.clone(),
            parsed_timeouts: self.parsed_timeouts.clone(),
            parsed_retry: self.parsed_retry.clone(),
            parsed_allow_non_idempotent_retry: self.parsed_allow_non_idempotent_retry,
        }
    }
}

impl fmt::Debug for GRPCRouteRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GRPCRouteRule")
            .field("name", &self.name)
            .field("matches", &self.matches)
            .field("filters", &self.filters)
            .field("backend_refs", &self.backend_refs)
            .field("timeouts", &self.timeouts)
            .field("retry", &self.retry)
            .field("session_persistence", &self.session_persistence)
            .field(
                "parsed_allow_non_idempotent_retry",
                &self.parsed_allow_non_idempotent_retry,
            )
            .finish()
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCRouteMatch {
    /// Method specifies a gRPC request service/method matcher
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<GRPCMethodMatch>,

    /// Headers specifies gRPC request header matchers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Vec<GRPCHeaderMatch>>,
}

/// Validated method predicates compiled once when building a route snapshot.
/// Exact values remain borrowed from the owning match; this is not a wire type.
#[derive(Clone, Debug)]
pub enum CompiledGrpcMethodMatch {
    Any,
    Exact,
    RegularExpression {
        service: Option<regex::Regex>,
        method: Option<regex::Regex>,
    },
}

impl GRPCRouteMatch {
    /// Apply the same Gateway API method policy on every configuration backend.
    /// Errors are bounded and never include operator-provided patterns.
    pub fn compile_method(&self) -> Result<CompiledGrpcMethodMatch, &'static str> {
        let Some(matcher) = &self.method else {
            return Ok(CompiledGrpcMethodMatch::Any);
        };
        let service = matcher.service.as_deref();
        let method = matcher.method.as_deref();
        if service.is_none_or(str::is_empty) && method.is_none_or(str::is_empty) {
            return Err("gRPC method matcher requires a non-empty service or method");
        }
        if [service, method]
            .into_iter()
            .flatten()
            .any(|value| value.chars().count() > 1024)
        {
            return Err("gRPC service and method must not exceed 1024 characters");
        }
        if matcher.match_type.as_ref() != Some(&GRPCMethodMatchType::RegularExpression) {
            // Mirror the vendored Gateway API Exact CEL name constraints,
            // including the optional leading dot on a service name.
            if service.is_some_and(|value| {
                !value
                    .strip_prefix('.')
                    .unwrap_or(value)
                    .split('.')
                    .all(valid_grpc_identifier)
            }) {
                return Err("gRPC Exact service must be a valid service name");
            }
            if method.is_some_and(|value| !valid_grpc_identifier(value)) {
                return Err("gRPC Exact method must be a valid method name");
            }
            return Ok(CompiledGrpcMethodMatch::Exact);
        }
        let compile = |pattern: Option<&str>| {
            pattern
                .filter(|pattern| !pattern.is_empty())
                .map(|pattern| {
                    regex::RegexBuilder::new(pattern)
                        .size_limit(10 * 1024 * 1024)
                        .dfa_size_limit(2 * 1024 * 1024)
                        .build()
                        .map_err(|_| "gRPC method matcher contains an invalid regular expression")
                })
                .transpose()
        };
        Ok(CompiledGrpcMethodMatch::RegularExpression {
            service: compile(service)?,
            method: compile(method)?,
        })
    }
}

fn valid_grpc_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

impl GRPCRouteRule {
    /// One invalid OR-branch invalidates its whole rule; valid sibling rules survive.
    pub fn method_match_error(&self) -> Option<&'static str> {
        self.matches
            .iter()
            .flatten()
            .find_map(|matcher| matcher.compile_method().err())
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCMethodMatch {
    /// Type specifies how to match against the service and method
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub match_type: Option<GRPCMethodMatchType>,

    /// Service is the name of the gRPC service
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,

    /// Method is the name of the gRPC method
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
}

/// GRPCMethodMatchType specifies how to match the method
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub enum GRPCMethodMatchType {
    /// Exact match
    Exact,
    /// RegularExpression match
    RegularExpression,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCHeaderMatch {
    /// Type specifies how to match the header
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub match_type: Option<String>,

    /// Name is the name of the gRPC Header
    #[schemars(
        length(max = crate::gwapi_types::HEADER_NAME_MAX_LEN),
        pattern(crate::gwapi_types::HEADER_NAME_PATTERN)
    )]
    pub name: String,

    /// Value is the value of gRPC Header
    pub value: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCBackendRef {
    /// Name is the name of the backend Service
    pub name: String,

    /// Namespace is the namespace of the backend Service
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Port specifies the destination port number
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = crate::gwapi_types::PORT_MIN, max = crate::gwapi_types::PORT_MAX))]
    pub port: Option<i32>,

    /// Weight specifies the proportion of requests forwarded to the backend
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<i32>,

    /// Group is the group of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,

    /// Kind is the kind of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,

    /// Filters defined at this level should be executed if and only if the request is being forwarded to this backend
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Vec<GRPCRouteFilter>>,

    /// Runtime-only BackendTLSPolicy attachment used by standalone backend
    /// utilities. Routed gRPC selection captures its request-stable policy
    /// separately and does not mutate this schema field.
    #[serde(skip)]
    #[schemars(skip)]
    pub backend_tls_policy: Option<Arc<crate::resources::BackendTLSPolicy>>,

    /// Cross-namespace reference denial info
    /// Set by Controller when this backend's cross-namespace reference
    /// is not permitted (no matching ReferenceGrant).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub ref_denied: Option<RefDenied>,
}

impl GRPCBackendRef {
    /// True if this backendRef delegates to a child GRPCRoute
    /// (`group: gateway.networking.k8s.io, kind: GRPCRoute`) instead of
    /// referencing a Service backend.
    pub fn is_route_delegation_ref(&self) -> bool {
        self.group.as_deref() == Some(GATEWAY_API_GROUP) && self.kind.as_deref() == Some("GRPCRoute")
    }
}

/// GRPCRouteFilter defines processing steps that must be completed during the request/response lifecycle
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCRouteFilter {
    /// Type identifies the type of filter to apply
    #[serde(rename = "type")]
    pub filter_type: GRPCRouteFilterType,

    /// RequestHeaderModifier defines a schema for a filter that modifies request headers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_header_modifier: Option<GRPCHeaderFilter>,

    /// ResponseHeaderModifier defines a schema for a filter that modifies response headers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_header_modifier: Option<GRPCHeaderFilter>,

    /// ExtensionRef is an optional, implementation-specific extension to the "filter" behavior
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_ref: Option<LocalObjectReference>,
}

/// GRPCRouteFilterType identifies a type of GRPCRoute filter
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub enum GRPCRouteFilterType {
    /// RequestHeaderModifier can be used to add or remove a gRPC header from a gRPC request
    RequestHeaderModifier,
    /// ResponseHeaderModifier can be used to add or remove a gRPC header from a gRPC response
    ResponseHeaderModifier,
    /// ExtensionRef is used for configuring custom gRPC plugins
    ExtensionRef,
}

/// GRPCHeaderFilter defines a filter that modifies the headers of a gRPC request or response
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCHeaderFilter {
    /// Set overwrites the request with the given header (name, value) before forwarding
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<Vec<HTTPHeader>>,

    /// Add adds the given header(s) (name, value) to the request before forwarding
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add: Option<Vec<HTTPHeader>>,

    /// Remove the given header(s) from the gRPC request before forwarding
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove: Option<Vec<String>>,
}

/// GRPCRouteTimeouts defines timeouts that can be configured for a GRPCRoute
/// Support: Extended
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCRouteTimeouts {
    /// Request specifies the maximum duration for a gateway to respond to a gRPC request
    /// This timeout covers the entire request-response transaction including all retries.
    /// Gateway API v1.4 standard field
    /// Format: Duration (e.g., "10s", "1m", "500ms")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,

    /// BackendRequest specifies a timeout for an individual request from the gateway
    /// to a backend. This covers the time from when the request first starts being
    /// sent from the gateway to when the full response has been received from the backend.
    /// Format: Duration (e.g., "10s", "1m", "500ms")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_request: Option<String>,
}

/// GRPCRouteRetry defines retry policy for a GRPCRoute
/// Support: Extended (Experimental)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GRPCRouteRetry {
    /// Attempts specifies the maximum number of times an individual request
    /// from the gateway to a backend should be retried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts: Option<i32>,

    /// Backoff specifies the minimum duration a Gateway should wait between
    /// retry attempts and is represented in Gateway API Duration formatting.
    /// Format: GEP-2257 Duration (e.g., "10ms", "1s")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub backoff: Option<String>,

    /// Codes specifies the gRPC status codes that a Gateway should
    /// retry for. When not specified, retries will be triggered only
    /// on connection errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codes: Option<Vec<i32>>,
}

// Note: ParsedRouteTimeouts is imported from http_route as it's shared between HTTP and gRPC routes

// ============================================
// Pre-parsing logic for GRPCRoute
// ============================================

/// Pre-parse methods for GRPCRoute.
impl GRPCRoute {
    /// Rules the gateway should compile: the controller-flattened delegation
    /// result when present, otherwise the operator-authored `rules`.
    pub fn effective_rules(&self) -> Option<&Vec<GRPCRouteRule>> {
        self.spec.resolved_rules.as_ref().or(self.spec.rules.as_ref())
    }

    /// Parse per-rule annotations (`retry-non-idempotent`).
    ///
    /// # Incomplete by itself
    ///
    /// `preparse()` does not parse timeouts or retry configurations.
    /// A fully pre-processed `GRPCRoute` requires all three calls:
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
        // Parse `edgion.io/retry-non-idempotent` annotation. Opt-in: "true"
        // re-enables replaying the request body on the reused-connection retry
        // path. gRPC is always POST at the HTTP layer, so the safe default
        // treats it as non-idempotent and suppresses that retry unless opted in.
        let parsed_allow_non_idempotent_retry: Option<bool> = self
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get(crate::constants::annotations::edgion::RETRY_NON_IDEMPOTENT))
            .and_then(|v| match v.as_str() {
                "true" => Some(true),
                "false" => Some(false),
                other => {
                    tracing::info!(
                        route_namespace = %self.metadata.namespace.as_deref().unwrap_or("default"),
                        route_name = %self.metadata.name.as_deref().unwrap_or(""),
                        annotation = %crate::constants::annotations::edgion::RETRY_NON_IDEMPOTENT,
                        value = %other,
                        "ignored: expected \"true\" or \"false\""
                    );
                    None
                }
            });

        // Both the operator-authored rules and the controller-flattened
        // delegation rules (resolved_rules) need the annotation copy; the
        // gateway compiles whichever is effective (see `effective_rules()`).
        for rules in [self.spec.rules.as_mut(), self.spec.resolved_rules.as_mut()]
            .into_iter()
            .flatten()
        {
            preparse_rules(rules, parsed_allow_non_idempotent_retry);
        }
    }

    /// Parse and pre-process timeout configurations for all rules
    ///
    /// This method is called during route loading (in pre_parse) to avoid runtime parsing overhead.
    /// It parses timeout strings into Duration objects and stores them in rule.parsed_timeouts.
    pub fn parse_timeouts(&mut self) {
        for rules in [self.spec.rules.as_mut(), self.spec.resolved_rules.as_mut()]
            .into_iter()
            .flatten()
        {
            parse_timeouts_rules(rules);
        }
    }

    /// Parse and pre-process retry configurations for all rules
    ///
    /// Only `attempts` and `backoff` are supported for GRPCRoute.
    /// `codes` (gRPC status codes) require trailers-level interception which is not yet implemented;
    /// if configured, a warning is logged and the field is ignored.
    pub fn parse_retry(&mut self) {
        let route_id = format!(
            "{}/{}",
            self.metadata.namespace.as_deref().unwrap_or("default"),
            self.metadata.name.as_deref().unwrap_or("")
        );

        for rules in [self.spec.rules.as_mut(), self.spec.resolved_rules.as_mut()]
            .into_iter()
            .flatten()
        {
            parse_retry_rules(rules, &route_id);
        }
    }
}

/// Copy the pre-parsed `retry-non-idempotent` annotation value onto every rule
/// in `rules`. Shared by `spec.rules` and `spec.resolved_rules`.
fn preparse_rules(rules: &mut [GRPCRouteRule], allow_non_idempotent: Option<bool>) {
    for rule in rules.iter_mut() {
        rule.parsed_allow_non_idempotent_retry = allow_non_idempotent;
    }
}

/// Parse and pre-process timeout configurations for `rules`. Shared by
/// `spec.rules` and `spec.resolved_rules`.
fn parse_timeouts_rules(rules: &mut [GRPCRouteRule]) {
    for rule in rules.iter_mut() {
        // Parse timeouts for each rule
        if let Some(timeouts) = &rule.timeouts {
            // Parse GRPCRouteTimeouts manually (same structure as HTTPRouteTimeouts)
            let request_timeout = timeouts
                .request
                .as_ref()
                .map(|s| HttpRouteTimeout::parse(s, "request"))
                .unwrap_or(HttpRouteTimeout::Unset);

            let backend_request_timeout = timeouts
                .backend_request
                .as_ref()
                .map(|s| HttpRouteTimeout::parse(s, "backend_request"))
                .unwrap_or(HttpRouteTimeout::Unset);

            // Clamp: backendRequest must be <= request (Gateway API spec)
            let backend_request_timeout = match (backend_request_timeout.as_duration(), request_timeout.as_duration()) {
                (Some(br), Some(req)) if br > req => {
                    tracing::warn!(
                        "GRPCRoute backendRequest timeout ({:?}) exceeds request timeout ({:?}), clamping",
                        br,
                        req
                    );
                    HttpRouteTimeout::Duration(req)
                }
                _ => backend_request_timeout,
            };

            if !matches!(request_timeout, HttpRouteTimeout::Unset)
                || !matches!(backend_request_timeout, HttpRouteTimeout::Unset)
            {
                rule.parsed_timeouts = Some(HttpParsedRouteTimeouts {
                    request_timeout,
                    backend_request_timeout,
                });

                tracing::debug!("Parsed route-level timeouts for GRPCRoute rule");
            }
        }
    }
}

/// Parse and pre-process retry configurations for `rules`. Shared by
/// `spec.rules` and `spec.resolved_rules`.
fn parse_retry_rules(rules: &mut [GRPCRouteRule], route_id: &str) {
    for rule in rules.iter_mut() {
        let Some(retry) = &rule.retry else {
            continue;
        };

        let attempts = retry.attempts.map(|a| a.max(0) as u32);

        let backoff = retry.backoff.as_deref().and_then(|s| {
            crate::duration::parse_duration(s)
                .map_err(|e| {
                    tracing::warn!(
                        route = %route_id,
                        value = %s,
                        error = %e,
                        "Invalid retry backoff duration in GRPCRoute, backoff disabled"
                    );
                })
                .ok()
        });

        // gRPC retry.codes are gRPC status codes (0-16), not HTTP status codes.
        // Status-code-based retry requires intercepting gRPC trailers (grpc-status),
        // which is not supported in the current Pingora proxy architecture.
        if retry.codes.as_ref().is_some_and(|c| !c.is_empty()) {
            tracing::warn!(
                route = %route_id,
                "GRPCRoute retry.codes is not yet supported (requires gRPC trailers interception); codes will be ignored"
            );
        }

        if attempts.is_some() || backoff.is_some() {
            rule.parsed_retry = Some(ParsedRouteRetry {
                attempts,
                backoff,
                codes: None, // Not supported for gRPC
            });
        }
    }
}

// ============================================================================
// GRPCRoute Status (Gateway API standard)
// ============================================================================

use super::http_route::RouteParentStatus;

/// GRPCRouteStatus describes the status of the GRPCRoute
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct GRPCRouteStatus {
    /// Parents describe the status of the route with respect to each parent.
    ///
    /// Always serialized, even when empty: K8s status persistence uses JSON
    /// Merge Patch, and merge-patching an empty status object `{}` cannot
    /// clear a previously-persisted non-empty list — only an explicit
    /// `"parents": []` replaces it (reachable when all parentRefs are removed
    /// from a live route). Do not add skip_serializing_if here.
    #[serde(default)]
    pub parents: Vec<RouteParentStatus>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn hostname_list_validation_is_optional_bounded_and_strict() {
        for (hostnames, valid) in [
            (serde_json::Value::Null, true),
            (serde_json::json!([]), true),
            (serde_json::json!(vec!["a"; 16]), true),
            (serde_json::json!(vec!["a"; 17]), false),
            (serde_json::json!(["a", "*"]), false),
            (serde_json::json!(["127.0.0.1"]), false),
            (serde_json::json!(["*.127.0.0.1"]), true),
        ] {
            let spec: super::GRPCRouteSpec =
                serde_json::from_value(serde_json::json!({"hostnames": hostnames})).unwrap();
            assert_eq!(spec.validate_hostnames().is_ok(), valid);
        }
    }
    use super::*;

    #[test]
    fn method_match_compile_policy_survives_json_and_yaml_roundtrip() {
        for (source, valid) in [
            ("{}", true),
            (r#"{"method":{"method":"Get"}}"#, true),
            (r#"{"method":{"type":"Exact","method":"Get"}}"#, true),
            (r#"{"method":{"service":".pkg.Service_1"}}"#, true),
            (r#"{"method":{"service":"pkg.Service","method":"_Get2"}}"#, true),
            (r#"{"method":{}}"#, false),
            (r#"{"method":{"type":"Exact"}}"#, false),
            (r#"{"method":{"service":"","method":"Get"}}"#, false),
            (r#"{"method":{"service":"pkg.Service","method":""}}"#, false),
            (r#"{"method":{"service":"pkg..Service"}}"#, false),
            (r#"{"method":{"service":"pkg.Service."}}"#, false),
            (r#"{"method":{"method":"1Get"}}"#, false),
            (r#"{"method":{"method":"Get/Item"}}"#, false),
            (r#"{"method":{"method":"Gét"}}"#, false),
            (r#"{"method":{"type":"RegularExpression","method":"^Get.*$"}}"#, true),
            (
                r#"{"method":{"type":"RegularExpression","service":"","method":"Get"}}"#,
                true,
            ),
            (
                r#"{"method":{"type":"RegularExpression","service":"svc","method":""}}"#,
                true,
            ),
            (
                r#"{"method":{"type":"RegularExpression","service":"","method":""}}"#,
                false,
            ),
            (
                r#"{"method":{"type":"RegularExpression","service":"[","method":"Get"}}"#,
                false,
            ),
            (
                r#"{"method":{"type":"RegularExpression","service":"svc","method":"["}}"#,
                false,
            ),
        ] {
            let matcher: GRPCRouteMatch = serde_json::from_str(source).unwrap();
            assert_eq!(matcher.compile_method().is_ok(), valid, "{source}");
            let wire = serde_json::to_value(&matcher).unwrap();
            let json_copy: GRPCRouteMatch = serde_json::from_value(wire.clone()).unwrap();
            let yaml_copy: GRPCRouteMatch = serde_yaml::from_str(&serde_yaml::to_string(&matcher).unwrap()).unwrap();
            assert_eq!(json_copy.compile_method().is_ok(), valid, "{source}");
            assert_eq!(yaml_copy.compile_method().is_ok(), valid, "{source}");
            assert_eq!(serde_json::to_value(yaml_copy).unwrap(), wire);
        }
    }

    #[test]
    fn compiled_method_distinguishes_any_exact_and_unconstrained_regex_fields() {
        let any: GRPCRouteMatch = serde_json::from_str("{}").unwrap();
        assert!(matches!(any.compile_method(), Ok(CompiledGrpcMethodMatch::Any)));
        let exact: GRPCRouteMatch = serde_json::from_str(r#"{"method":{"method":"Get"}}"#).unwrap();
        assert!(matches!(exact.compile_method(), Ok(CompiledGrpcMethodMatch::Exact)));
        let regex: GRPCRouteMatch =
            serde_json::from_str(r#"{"method":{"type":"RegularExpression","service":"","method":"^Get.*$"}}"#).unwrap();
        let CompiledGrpcMethodMatch::RegularExpression { service, method } = regex.compile_method().unwrap() else {
            panic!("expected regex matcher");
        };
        assert!(service.is_none());
        let method = method.unwrap();
        assert!(method.is_match("GetItem"));
        assert!(!method.is_match("DeleteItem"));
    }

    #[test]
    fn method_match_length_and_rule_failure_boundaries() {
        for match_type in ["Exact", "RegularExpression"] {
            for (length, valid) in [(1024, true), (1025, false)] {
                let matcher: GRPCRouteMatch = serde_json::from_value(serde_json::json!({
                    "method": {"type": match_type, "method": "a".repeat(length)}
                }))
                .unwrap();
                assert_eq!(matcher.compile_method().is_ok(), valid);
            }
        }
        let rule: GRPCRouteRule = serde_json::from_value(serde_json::json!({"matches": [
            {"method": {"method": "Get"}},
            {"method": {"type": "RegularExpression", "method": "private-marker["}}
        ]}))
        .unwrap();
        let error = rule.method_match_error().unwrap();
        assert_eq!(error, "gRPC method matcher contains an invalid regular expression");
        assert!(!error.contains("private-marker"));
        let catch_all: GRPCRouteRule = serde_json::from_str("{}").unwrap();
        assert!(catch_all.method_match_error().is_none());
    }

    fn backend_ref(group: Option<&str>, kind: Option<&str>) -> GRPCBackendRef {
        GRPCBackendRef {
            name: "target".to_string(),
            namespace: None,
            port: None,
            weight: None,
            group: group.map(str::to_string),
            kind: kind.map(str::to_string),
            filters: None,
            backend_tls_policy: None,
            ref_denied: None,
        }
    }

    #[test]
    fn test_is_route_delegation_ref_matrix() {
        // Positive: exact group + kind
        assert!(backend_ref(Some(GATEWAY_API_GROUP), Some("GRPCRoute")).is_route_delegation_ref());
        // Negative: kind absent (defaults to Service semantics)
        assert!(!backend_ref(Some(GATEWAY_API_GROUP), None).is_route_delegation_ref());
        // Negative: Service kind
        assert!(!backend_ref(None, Some("Service")).is_route_delegation_ref());
        // Negative: right kind, wrong/absent group (core group Service named GRPCRoute)
        assert!(!backend_ref(None, Some("GRPCRoute")).is_route_delegation_ref());
        assert!(!backend_ref(Some("example.io"), Some("GRPCRoute")).is_route_delegation_ref());
        // Negative: HTTPRoute is not a gRPC delegation target
        assert!(!backend_ref(Some(GATEWAY_API_GROUP), Some("HTTPRoute")).is_route_delegation_ref());
    }

    fn route_json(spec: serde_json::Value) -> GRPCRoute {
        serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": {"name": "r", "namespace": "default"},
            "spec": spec
        }))
        .expect("route should deserialize")
    }

    #[test]
    fn test_effective_rules_prefers_resolved() {
        let mut route = route_json(serde_json::json!({
            "rules": [{"matches": [{"method": {"service": "svc.Spec"}}]}]
        }));
        // No resolved_rules → falls back to spec.rules
        let rules = route.effective_rules().expect("spec rules present");
        assert_eq!(
            rules[0].matches.as_ref().unwrap()[0]
                .method
                .as_ref()
                .unwrap()
                .service
                .as_deref(),
            Some("svc.Spec")
        );

        // With resolved_rules → resolved wins
        let resolved: Vec<GRPCRouteRule> = serde_json::from_value(serde_json::json!(
            [{"matches": [{"method": {"service": "svc.Resolved"}}]}]
        ))
        .unwrap();
        route.spec.resolved_rules = Some(resolved);
        let rules = route.effective_rules().expect("resolved rules present");
        assert_eq!(
            rules[0].matches.as_ref().unwrap()[0]
                .method
                .as_ref()
                .unwrap()
                .service
                .as_deref(),
            Some("svc.Resolved")
        );
    }

    #[test]
    fn test_resolved_rules_serialized_delegation_issues_not() {
        let mut route = route_json(serde_json::json!({"rules": []}));
        route.spec.resolved_rules = Some(Vec::new());
        route.spec.delegation_issues = vec![DelegationIssue::RuleDropped {
            message: "internal".to_string(),
        }];
        let json = serde_json::to_value(&route).unwrap();
        // resolved_rules must survive conf_sync serialization
        assert!(json["spec"].get("resolvedRules").is_some());
        // delegation_issues is controller-internal and must never serialize
        assert!(json["spec"].get("delegationIssues").is_none());
        assert!(json["spec"].get("delegation_issues").is_none());
    }

    #[test]
    fn test_resolved_rules_only_covered_by_preparse_pipeline() {
        // A route with ONLY resolved_rules set (no spec.rules) must still get
        // full load-time processing: timeouts, retry, and annotation copies.
        let mut route = route_json(serde_json::json!({}));
        route.spec.resolved_rules = Some(
            serde_json::from_value(serde_json::json!([{
                "matches": [{"method": {"service": "svc.Billing"}}],
                "timeouts": {"request": "10s"},
                "retry": {"attempts": 2}
            }]))
            .unwrap(),
        );

        route.preparse();
        route.parse_timeouts();
        route.parse_retry();

        let rule = &route.spec.resolved_rules.as_ref().unwrap()[0];
        assert!(rule.parsed_timeouts.is_some(), "resolved_rules timeouts must be parsed");
        assert_eq!(
            rule.parsed_retry.as_ref().and_then(|r| r.attempts),
            Some(2),
            "resolved_rules retry must be parsed"
        );
        // No annotation set on the route → None, but the field must still be
        // reachable/populated by the preparse pipeline (not left stale).
        assert_eq!(
            rule.parsed_allow_non_idempotent_retry, None,
            "resolved_rules annotation copy must run (absent annotation -> None)"
        );
    }

    #[test]
    fn test_resolved_rules_annotation_copy_reaches_resolved_rules() {
        let mut route: GRPCRoute = serde_json::from_value(serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "GRPCRoute",
            "metadata": {
                "name": "parent", "namespace": "infra",
                "annotations": {"edgion.io/retry-non-idempotent": "true"}
            },
            "spec": {
                "rules": [{"matches": [{"method": {"service": "svc.Parent"}}]}]
            }
        }))
        .unwrap();
        route.spec.resolved_rules = Some(
            serde_json::from_value(serde_json::json!([{
                "matches": [{"method": {"service": "svc.Child"}}]
            }]))
            .unwrap(),
        );

        route.preparse();

        let rule = &route.spec.resolved_rules.as_ref().unwrap()[0];
        assert_eq!(
            rule.parsed_allow_non_idempotent_retry,
            Some(true),
            "annotation copies must reach resolved_rules"
        );
    }
}
