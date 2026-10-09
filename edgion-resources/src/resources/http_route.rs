//! HTTPRoute resource definition
//!
//! HTTPRoute defines HTTP rules for mapping requests to backends

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use super::common::{parse_byte_size, Condition, ParentReference, RefDenied};
use crate::ListenerKey;

/// HTTPRoute defines HTTP rules for mapping requests to backends
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "HTTPRoute",
    plural = "httproutes",
    status = "HTTPRouteStatus",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteSpec {
    /// Controller-computed status for this deployment, transported to the Gateway.
    /// Never supplied by operators or used as the persisted status history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub current_status: Option<HTTPRouteStatus>,

    /// ParentRefs references the resources that this Route wants to be attached to
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<ParentReference>>,

    /// Experimental default Gateway attachment is not implemented. Retain the
    /// source value so the controller can report it on this object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub use_default_gateways: Option<String>,

    /// Controller-owned status identity, published with this processed route by conf_sync.
    /// Overwritten by the Controller; never an operator-authored field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_status_controller: Option<String>,

    /// Hostnames defines the set of hostnames
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

    /// Rules defines the HTTP routing rules
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<HTTPRouteRule>>,

    /// Controller-resolved flattened rules produced by HTTPRoute delegation
    /// (a rule whose backendRef points at `kind: HTTPRoute` delegates its path
    /// prefix to child routes; the controller expands the delegation tree).
    /// Present only on routes containing at least one delegating rule. When
    /// present, the gateway compiles these INSTEAD of `rules`, and
    /// `invalid_rule_indices` does not apply (flattening already dropped
    /// invalid rules). See docs/en/user-guide/http-route/delegation.md and
    /// skills/01-architecture/05-resources/04-http-route.md.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_rules: Option<Vec<HTTPRouteRule>>,

    /// Controller-internal delegation diagnostics, produced by the flatten
    /// step in `parse` and consumed by `update_status`. Never serialized.
    #[serde(skip)]
    #[schemars(skip)]
    pub delegation_issues: Vec<DelegationIssue>,
}

/// v1beta1 wire source. Nested Standard and selected Experimental fields use
/// the same typed model as v1; only controller-resolved fields are omitted.
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1beta1",
    kind = "HTTPRoute",
    root = "HTTPRouteBeta",
    plural = "httproutes",
    status = "HTTPRouteStatus",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteBetaSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_refs: Option<Vec<ParentReference>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostnames: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<HTTPRouteRule>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_default_gateways: Option<String>,
}

impl From<HTTPRouteBeta> for HTTPRoute {
    fn from(beta: HTTPRouteBeta) -> Self {
        Self {
            metadata: beta.metadata,
            spec: HTTPRouteSpec {
                current_status: None,
                parent_refs: beta.spec.parent_refs,
                resolved_status_controller: None,
                hostnames: beta.spec.hostnames,
                resolved_hostnames: None,
                resolved_listeners: None,
                invalid_rule_indices: None,
                rules: beta.spec.rules,
                resolved_rules: None,
                delegation_issues: Vec::new(),
                use_default_gateways: beta.spec.use_default_gateways,
            },
            status: beta.status,
        }
    }
}

impl HTTPRouteSpec {
    /// Validate authored hostname syntax and the Gateway API list bound.
    pub fn validate_hostnames(&self) -> Result<(), crate::HostnameError> {
        crate::validate_route_hostnames(self.hostnames.as_deref(), false)
    }

    pub fn unsupported_default_gateway_attachment(&self) -> Option<&'static str> {
        self.use_default_gateways
            .as_deref()
            .is_some_and(|scope| scope != "None")
            .then_some("useDefaultGateways attachment is not supported")
    }
}

/// Diagnostics from delegation flattening (controller-internal).
#[derive(Clone, Debug, PartialEq)]
pub enum DelegationIssue {
    /// A named child HTTPRoute referenced by a delegating rule does not exist
    /// (surfaces as `ResolvedRefs=False` / `BackendNotFound`).
    MissingChild { namespace: String, name: String },
    /// A delegating rule or a child rule was dropped from `resolved_rules`
    /// (surfaces via the `PartiallyInvalid` condition).
    RuleDropped { message: String },
}

/// API group of the Gateway API resources (used for delegation ref detection).
pub const GATEWAY_API_GROUP: &str = "gateway.networking.k8s.io";
/// API group of Edgion custom resources.
pub const EDGION_API_GROUP: &str = "edgion.io";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteRule {
    /// Controller-resolved source facts for scoped Gateway AI admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_ai_admission: Option<super::http_route_ai_admission::ResolvedAiAdmission>,

    /// Name identifies this rule for observability and status correlation
    /// Support: Extended
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Matches define conditions used for matching the rule against requests
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches: Option<Vec<HTTPRouteMatch>>,

    /// Filters define the plugins that are applied to requests
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Vec<HTTPRouteFilter>>,

    /// BackendRefs defines the backend(s) where matching requests should be sent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_refs: Option<Vec<HTTPBackendRef>>,

    /// Timeouts defines timeouts for requests matching this rule
    /// Support: Extended
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeouts: Option<HTTPRouteTimeouts>,

    /// Retry defines the retry policy for requests matching this rule
    /// Support: Extended (Experimental)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<HTTPRouteRetry>,

    /// SessionPersistence defines the session persistence configuration
    /// Support: Extended (Experimental)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_persistence: Option<SessionPersistence>,

    /// Controller-resolved identity of the terminal HTTPRoute that owns this
    /// effective rule. Kubernetes resources use `metadata.uid`; UID-less
    /// config-center resources use the stable typed route key instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_terminal_route_uid: Option<String>,

    /// Controller-resolved identity of the terminal rule within its owning
    /// route. Named rules use `name:<value>` and unnamed rules use
    /// `index:<zero-based index>` so the two domains cannot collide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_terminal_rule_identity: Option<String>,

    /// Pre-parsed route-level timeout configurations (not serialized)
    /// Parsed once when route is loaded, used at runtime
    #[serde(skip)]
    #[schemars(skip)]
    pub parsed_timeouts: Option<ParsedRouteTimeouts>,

    /// Pre-parsed retry configuration from HTTPRouteRule.retry (not serialized)
    /// Parsed once when route is loaded, used at runtime
    #[serde(skip)]
    #[schemars(skip)]
    pub parsed_retry: Option<ParsedRouteRetry>,

    /// Pre-parsed `edgion.io/forward-raw-path` annotation from parent HTTPRoute.metadata.
    ///   - `None` — annotation absent (default flow)
    ///   - `Some(true)` — annotation `"true"` (force raw forward)
    ///   - `Some(false)` — annotation `"false"` (force normalized forward, overrides
    ///     region_route's `preserve_raw_path`)
    ///
    /// Parsed in `http_route_preparse.rs` at HTTPRoute load time and copied to
    /// every rule under that route. Invalid annotation values are logged at INFO
    /// and treated as `None`.
    #[serde(skip)]
    #[schemars(skip)]
    pub parsed_forward_raw_path: Option<bool>,

    /// Pre-parsed `edgion.io/retry-non-idempotent` annotation from parent
    /// HTTPRoute.metadata. `Some(true)` opts this route's reused-connection
    /// retry into replaying non-idempotent request bodies; `None` (annotation
    /// absent or invalid) keeps the safe default where non-idempotent methods
    /// are not retried on a reused-connection reset.
    ///
    /// Parsed in `http_route_preparse.rs` at HTTPRoute load time and copied to
    /// every rule under that route.
    #[serde(skip)]
    #[schemars(skip)]
    pub parsed_allow_non_idempotent_retry: Option<bool>,

    /// Pre-parsed `edgion.io/mirror-*` tuning annotations from parent
    /// HTTPRoute.metadata, applied to every RequestMirror filter under this
    /// route. `None` — no mirror annotation present (defaults apply).
    ///
    /// Lives in annotations (not on `HTTPRequestMirrorFilter`) because the
    /// vendored Gateway API CRD silently prunes undeclared filter fields in
    /// K8s conf-center mode. Parsed in `http_route_preparse.rs` at load time
    /// and copied to every rule under that route.
    #[serde(skip)]
    #[schemars(skip)]
    pub parsed_mirror_tuning: Option<MirrorTuning>,
}

impl HTTPRouteRule {
    /// Return the first invalid standard RequestMirror configuration in this
    /// rule, including filters attached to an individual backendRef.
    pub fn request_mirror_validation_error(&self) -> Option<String> {
        let validate = |filter: &HTTPRouteFilter| match (
            filter.filter_type == HTTPRouteFilterType::RequestMirror,
            filter.request_mirror.as_ref(),
        ) {
            (true, Some(config)) => config.validation_error(),
            (true, None) => Some("requestMirror must be specified when filter.type is RequestMirror".to_string()),
            (false, Some(_)) => Some("requestMirror must be absent when filter.type is not RequestMirror".to_string()),
            (false, None) => None,
        };

        if let Some((index, error)) = self
            .filters
            .iter()
            .flatten()
            .enumerate()
            .find_map(|(index, filter)| validate(filter).map(|error| (index, error)))
        {
            return Some(format!("filters[{index}] RequestMirror: {error}"));
        }

        self.backend_refs
            .iter()
            .flatten()
            .enumerate()
            .find_map(|(backend_index, backend)| {
                backend
                    .filters
                    .iter()
                    .flatten()
                    .enumerate()
                    .find_map(|(filter_index, filter)| {
                        validate(filter).map(|error| {
                            format!("backendRefs[{backend_index}].filters[{filter_index}] RequestMirror: {error}")
                        })
                    })
            })
    }

    /// Invalid or unsupported native CORS policies invalidate the whole rule
    /// before Controller delegation or Gateway publication.
    pub fn unsupported_cors_reason(&self) -> Option<String> {
        self.unsupported_backend_cors_reason().or_else(|| {
            let filters = self.filters.as_ref()?;
            if filters
                .iter()
                .filter(|filter| filter.filter_type == HTTPRouteFilterType::CORS)
                .take(2)
                .count()
                > 1
            {
                return Some("CORS filter cannot be repeated".to_string());
            }
            filters.iter().find_map(|filter| {
                if filter.filter_type != HTTPRouteFilterType::CORS {
                    return None;
                }
                match filter.cors.as_ref() {
                    None => Some("CORS filter missing 'cors' configuration block".to_string()),
                    Some(config) => config
                        .validate()
                        .err()
                        .map(|errors| format!("CORS {}", errors.join("; "))),
                }
            })
        })
    }

    /// Native CORS is supported only at rule scope. Reject the whole rule when
    /// any backend reference carries it, even at weight zero or alongside a
    /// rule-level policy. Preflight ownership cannot depend on backend selection.
    /// Shared by Controller validation (including delegation) and Gateway publication.
    pub fn unsupported_backend_cors_reason(&self) -> Option<String> {
        let index = self.backend_refs.as_ref()?.iter().position(|backend| {
            backend.filters.as_ref().is_some_and(|filters| {
                filters
                    .iter()
                    .any(|filter| filter.filter_type == HTTPRouteFilterType::CORS)
            })
        })?;
        Some(format!(
            "backendRefs[{index}] CORS is not supported; configure CORS in rules[].filters instead"
        ))
    }
}

impl Clone for HTTPRouteRule {
    fn clone(&self) -> Self {
        Self {
            resolved_ai_admission: self.resolved_ai_admission.clone(),
            name: self.name.clone(),
            matches: self.matches.clone(),
            filters: self.filters.clone(),
            backend_refs: self.backend_refs.clone(),
            timeouts: self.timeouts.clone(),
            retry: self.retry.clone(),
            session_persistence: self.session_persistence.clone(),
            resolved_terminal_route_uid: self.resolved_terminal_route_uid.clone(),
            resolved_terminal_rule_identity: self.resolved_terminal_rule_identity.clone(),
            parsed_timeouts: self.parsed_timeouts.clone(),
            parsed_retry: self.parsed_retry.clone(),
            parsed_forward_raw_path: self.parsed_forward_raw_path,
            parsed_allow_non_idempotent_retry: self.parsed_allow_non_idempotent_retry,
            parsed_mirror_tuning: self.parsed_mirror_tuning.clone(),
        }
    }
}

impl fmt::Debug for HTTPRouteRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HTTPRouteRule")
            .field("name", &self.name)
            .field("matches", &self.matches)
            .field("plugins", &self.filters)
            .field("backend_refs", &self.backend_refs)
            .field("timeouts", &self.timeouts)
            .field("retry", &self.retry)
            .field("session_persistence", &self.session_persistence)
            .field("resolved_terminal_route_uid", &self.resolved_terminal_route_uid)
            .field("resolved_terminal_rule_identity", &self.resolved_terminal_rule_identity)
            .field("parsed_forward_raw_path", &self.parsed_forward_raw_path)
            .field(
                "parsed_allow_non_idempotent_retry",
                &self.parsed_allow_non_idempotent_retry,
            )
            .finish()
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteMatch {
    /// Path specifies a HTTP request path matcher.
    ///
    /// Omitting it means `{type: PathPrefix, value: "/"}` per the Gateway API
    /// spec. On Kubernetes the API server backfills that value from the
    /// structural schema at admission, but the File and Etcd config-center
    /// backends never pass through admission — so the default is replicated
    /// here to keep every backend on the same semantics. Without it a
    /// spec-valid header/method-only match compiles into a route unit with no
    /// radix-tree entry and becomes permanently unreachable.
    #[serde(
        default = "default_path_prefix_root",
        deserialize_with = "deserialize_path_match_or_default",
        skip_serializing_if = "Option::is_none"
    )]
    pub path: Option<HTTPPathMatch>,

    /// Header matchers, retaining only the first ASCII-case-equivalent name.
    /// Normalize at decode before Controller validation/delegation and Gateway
    /// regex compilation, predicate evaluation, and specificity counting.
    #[serde(
        default,
        deserialize_with = "deserialize_header_matches",
        skip_serializing_if = "Option::is_none"
    )]
    pub headers: Option<Vec<HTTPHeaderMatch>>,

    /// QueryParams specifies HTTP query parameter matchers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_params: Option<Vec<HTTPQueryParamMatch>>,

    /// Method specifies HTTP method matcher
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
}

fn deserialize_header_matches<'de, D>(deserializer: D) -> Result<Option<Vec<HTTPHeaderMatch>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mut headers = Option::<Vec<HTTPHeaderMatch>>::deserialize(deserializer)?;
    if let Some(headers) = headers.as_mut().filter(|headers| headers.len() > 1) {
        let mut seen = HashSet::with_capacity(headers.len());
        headers.retain(|header| seen.insert(header.name.to_ascii_lowercase()));
    }
    Ok(headers)
}

/// Gateway API structural default for an omitted `HTTPRouteMatch.path`.
fn default_path_prefix_root() -> Option<HTTPPathMatch> {
    Some(super::request_transform::default_path_match())
}

fn deserialize_path_match_or_default<'de, D>(deserializer: D) -> Result<Option<HTTPPathMatch>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<HTTPPathMatch>::deserialize(deserializer)?.or_else(default_path_prefix_root))
}

fn deserialize_path_type_or_default<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.or_else(super::request_transform::default_path_match_type))
}

fn deserialize_path_value_or_default<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.or_else(super::request_transform::default_path_match_value))
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPPathMatch {
    /// Type specifies how to match against the path value
    #[serde(
        rename = "type",
        default = "super::request_transform::default_path_match_type",
        deserialize_with = "deserialize_path_type_or_default",
        skip_serializing_if = "Option::is_none"
    )]
    pub match_type: Option<String>,

    /// Value of the HTTP path to match against.
    ///
    /// For `Exact` and `PathPrefix` types the value may also contain named
    /// path parameters using the `:name` syntax (Edgion extension to the
    /// Gateway API spec). Examples:
    ///   - `/users/:id` — captures `id` for the second segment
    ///   - `/api/:version/users/:uid/posts/:postId` — captures multiple
    ///   - `::literal` — escape: literal `:literal` segment, not a parameter
    ///
    /// Captured values are exposed at runtime through the DSL builtin
    /// `req.path_param("name")` and via `$name` template substitution in the
    /// `ProxyRewrite` plugin (URI and header values). They are not consumed
    /// by Gateway API standard filters (`URLRewrite`, `RequestRedirect`,
    /// `RequestHeaderModifier`, ...), which keep their literal-string
    /// semantics defined by the spec.
    ///
    /// `RegularExpression` paths do not participate in `:name` extraction;
    /// use regex capture groups (`$1`-`$9`) in `ProxyRewrite` for that case.
    #[serde(
        default = "super::request_transform::default_path_match_value",
        deserialize_with = "deserialize_path_value_or_default",
        skip_serializing_if = "Option::is_none"
    )]
    pub value: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPHeaderMatch {
    /// Type specifies how to match the header
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub match_type: Option<String>,

    /// Name is the name of the HTTP Header
    #[schemars(
        length(max = crate::gwapi_types::HEADER_NAME_MAX_LEN),
        pattern(crate::gwapi_types::HEADER_NAME_PATTERN)
    )]
    pub name: String,

    /// Value is the value of HTTP Header
    pub value: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPQueryParamMatch {
    /// Type specifies how to match the query parameter
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub match_type: Option<String>,

    /// Name is the name of the query parameter
    pub name: String,

    /// Value is the value of the query parameter
    pub value: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPBackendRef {
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
    pub filters: Option<Vec<HTTPRouteFilter>>,

    /// Runtime-only BackendTLSPolicy attachment used by standalone backend
    /// utilities. Routed Gateway HTTP selection captures its request-stable
    /// policy separately and does not mutate this schema field.
    #[serde(skip)]
    #[schemars(skip)]
    pub backend_tls_policy: Option<Arc<crate::resources::BackendTLSPolicy>>,

    /// Cross-namespace reference denial info
    /// Set by Controller when this backend's cross-namespace reference
    /// is not permitted (no matching ReferenceGrant).
    /// When present, Gateway will reject requests to this backend
    /// and log the denial details to access log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub ref_denied: Option<RefDenied>,
}

/// Stable identity of one declaration in an effective HTTPRoute backendRefs
/// list. `occurrence` is counted among equal normalized tuples, while Gateway
/// keeps the raw vector slot separately for dispatch.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogicalBackendSlotIdentity {
    pub group: String,
    pub kind: String,
    pub namespace: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    pub occurrence: u16,
}

impl LogicalBackendSlotIdentity {
    pub fn matches_ai_selector(&self, selector: &crate::resources::edgion_plugins::AiBackendSlotSelector) -> bool {
        self.group == selector.group
            && self.kind == selector.kind
            && self.namespace == selector.namespace
            && self.name == selector.name
            && self.section == selector.section
            && self.occurrence == selector.occurrence
    }
}

/// Construct one canonical identity for every declaration in order.
pub fn logical_backend_slot_identities(
    refs: &[HTTPBackendRef],
    route_namespace: &str,
) -> Vec<LogicalBackendSlotIdentity> {
    let mut occurrences: HashMap<(String, String, String, String, Option<String>), u16> = HashMap::new();
    refs.iter()
        .map(|backend| {
            let group = super::refs::targets::backend_group(backend.group.as_deref()).to_string();
            let kind = super::refs::targets::backend_kind(backend.kind.as_deref()).to_string();
            let namespace = backend.namespace.as_deref().unwrap_or(route_namespace).to_string();
            let tuple = (
                group.clone(),
                kind.clone(),
                namespace.clone(),
                backend.name.clone(),
                None,
            );
            let occurrence = occurrences.entry(tuple).or_default();
            let identity = LogicalBackendSlotIdentity {
                group,
                kind,
                namespace,
                name: backend.name.clone(),
                section: None,
                occurrence: *occurrence,
            };
            *occurrence = occurrence.saturating_add(1);
            identity
        })
        .collect()
}

#[cfg(test)]
#[path = "../../tests/unit/resources/http_route_test/logical_slot_identity_tests.rs"]
mod logical_slot_identity_tests;

/// Canonical classification of an HTTPRoute backendRef.
///
/// Keep this classification in the shared resource layer so the Controller
/// and Gateway cannot disagree about which downstream subsystem owns a ref.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HTTPBackendRefKind {
    /// A Kubernetes Service backend (Gateway API default or an Edgion Service-family
    /// kind such as `ServiceExternalName` / `ServiceClusterIp`). Peer selection still
    /// Transport selection follows the actual Service and its endpoints.
    Service,
    /// An Edgion AI provider-service resource.
    EdgionBackend,
    /// A child HTTPRoute expanded by Controller-side route delegation.
    RouteDelegation,
    /// Any other group/kind combination.
    Unsupported,
}

impl HTTPBackendRef {
    /// Classify this backendRef using Gateway API defaults and Edgion's exact
    /// extension groups/kinds.
    pub fn backend_kind(&self) -> HTTPBackendRefKind {
        match (self.group.as_deref(), self.kind.as_deref()) {
            (group, kind) if super::refs::targets::is_standard_service(group, kind) => HTTPBackendRefKind::Service,
            (Some(EDGION_API_GROUP), Some("EdgionBackend")) => HTTPBackendRefKind::EdgionBackend,
            (Some(GATEWAY_API_GROUP), Some("HTTPRoute")) => HTTPBackendRefKind::RouteDelegation,
            _ => HTTPBackendRefKind::Unsupported,
        }
    }

    /// True if this backendRef delegates to a child HTTPRoute
    /// (`group: gateway.networking.k8s.io, kind: HTTPRoute`) instead of
    /// referencing a Service backend.
    pub fn is_route_delegation_ref(&self) -> bool {
        self.backend_kind() == HTTPBackendRefKind::RouteDelegation
    }
}

impl HTTPRoute {
    /// Rules the gateway should compile: the controller-flattened delegation
    /// result when present, otherwise the operator-authored `rules`.
    pub fn effective_rules(&self) -> Option<&Vec<HTTPRouteRule>> {
        self.spec.resolved_rules.as_ref().or(self.spec.rules.as_ref())
    }
}

/// HTTPRouteFilter defines processing steps that must be completed during the request/response lifecycle
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteFilter {
    /// Type identifies the type of filter to apply
    #[serde(rename = "type")]
    pub filter_type: HTTPRouteFilterType,

    /// RequestHeaderModifier defines a schema for a filter that modifies request headers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_header_modifier: Option<HTTPHeaderFilter>,

    /// ResponseHeaderModifier defines a schema for a filter that modifies response headers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_header_modifier: Option<HTTPHeaderFilter>,

    /// RequestRedirect defines a schema for a filter that responds to the request with an HTTP redirection
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_redirect: Option<HTTPRequestRedirectFilter>,

    /// URLRewrite defines a schema for a filter that modifies a request during forwarding
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_rewrite: Option<HTTPURLRewriteFilter>,

    /// RequestMirror defines a schema for a filter that mirrors requests
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_mirror: Option<HTTPRequestMirrorFilter>,

    /// CORS defines a Gateway API native CORS filter (GEP-1767)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cors: Option<HTTPCORSFilter>,

    /// ExternalAuth preserves native GEP-1494 input separately from internal Edgion auth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_auth: Option<HTTPRouteExternalAuth>,

    /// Controller-derived locality and failure state for the internal Edgion auth filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_external_auth: Option<crate::resources::refs::route_auth_fields::ResolvedExternalAuth>,

    /// ExtensionRef is an optional, implementation-specific extension to the "filter" behavior
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_ref: Option<LocalObjectReference>,
}

/// The upstream v1.5 ExternalAuth schema is an Envoy ext_authz reference,
/// while Edgion's earlier ForwardAuth object is an internal extension. Keep
/// the upstream object intact so one unsupported rule can be status-reported
/// without failing deserialization of its valid siblings.
#[derive(Serialize, Clone, Debug, JsonSchema)]
#[serde(untagged)]
pub enum HTTPRouteExternalAuth {
    Edgion(Box<crate::resources::edgion_plugins::ForwardAuthConfig>),
    Upstream(serde_json::Value),
}

impl<'de> Deserialize<'de> for HTTPRouteExternalAuth {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        // Protocol and backendRef identify the upstream Envoy ext_authz shape.
        // Check them before attempting Edgion's permissive ForwardAuth decoder:
        // a mixed object with a `decision` must never lose the upstream keys.
        if value.as_object().is_some_and(|object| {
            ["backendRef", "protocol", "http", "grpc"]
                .iter()
                .any(|key| object.contains_key(*key))
        }) {
            return Ok(Self::Upstream(value));
        }
        match serde_json::from_value(value.clone()) {
            Ok(config) => Ok(Self::Edgion(Box::new(config))),
            Err(_) => Ok(Self::Upstream(value)),
        }
    }
}

impl HTTPRouteExternalAuth {
    pub fn validation_error(&self) -> Option<String> {
        match self {
            Self::Edgion(config) => config.validation_error(),
            Self::Upstream(_) => {
                Some("Gateway API ExternalAuth backendRef/protocol is not supported by Edgion ForwardAuth".to_string())
            }
        }
    }

    pub fn edgion_config(&self) -> Option<&crate::resources::edgion_plugins::ForwardAuthConfig> {
        match self {
            Self::Edgion(config) => Some(config.as_ref()),
            Self::Upstream(_) => None,
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/resources/http_route_test/source_version_tests.rs"]
mod source_version_tests;

/// HTTPRouteFilterType identifies a type of HTTPRoute filter
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub enum HTTPRouteFilterType {
    /// RequestHeaderModifier can be used to add or remove an HTTP header from an HTTP request
    RequestHeaderModifier,
    /// ResponseHeaderModifier can be used to add or remove an HTTP header from an HTTP response
    ResponseHeaderModifier,
    /// RequestRedirect can be used to redirect a request to another location
    RequestRedirect,
    /// URLRewrite can be used to modify a request during forwarding
    URLRewrite,
    /// RequestMirror can be used to mirror HTTP requests to a different backend
    RequestMirror,
    /// ExtensionRef is used for configuring custom HTTP plugins
    ExtensionRef,
    /// CORS defines a Gateway API native CORS filter (GEP-1767)
    CORS,
    /// ExternalAuth preserves native GEP-1494 input separately from internal Edgion auth.
    /// Only the internal Edgion payload maps to ForwardAuth; native input is unsupported.
    ExternalAuth,
}

/// HTTPHeaderFilter defines a filter that modifies the headers of an HTTP request or response
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPHeaderFilter {
    /// Set overwrites the request with the given header (name, value) before forwarding
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<Vec<HTTPHeader>>,

    /// Add adds the given header(s) (name, value) to the request before forwarding
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add: Option<Vec<HTTPHeader>>,

    /// Remove the given header(s) from the HTTP request before forwarding
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove: Option<Vec<String>>,
}

/// HTTPHeader represents an HTTP Header name and value as defined by RFC 7230
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
pub struct HTTPHeader {
    /// Name is the name of the HTTP Header
    #[schemars(
        length(max = crate::gwapi_types::HEADER_NAME_MAX_LEN),
        pattern(crate::gwapi_types::HEADER_NAME_PATTERN)
    )]
    pub name: String,
    /// Value is the value of HTTP Header
    pub value: String,
}

/// HTTPRequestRedirectFilter defines a filter that redirects a request
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRequestRedirectFilter {
    /// Scheme is the scheme to be used in the value of the Location header
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,

    /// Hostname is the hostname to be used in the value of the Location header
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        length(max = crate::gwapi_types::HOSTNAME_MAX_LEN),
        pattern(crate::gwapi_types::PRECISE_HOSTNAME_PATTERN)
    )]
    pub hostname: Option<String>,

    /// Path defines parameters used to modify the path of the incoming request
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<HTTPPathModifier>,

    /// Port is the port to be used in the value of the Location header
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = crate::gwapi_types::PORT_MIN, max = crate::gwapi_types::PORT_MAX))]
    pub port: Option<i32>,

    /// StatusCode is the HTTP status code to be used in response
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<i32>,
}

/// HTTPURLRewriteFilter defines a filter that modifies a request during forwarding
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPURLRewriteFilter {
    /// Hostname is the value to be used to replace the Host header value during forwarding
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        length(max = crate::gwapi_types::HOSTNAME_MAX_LEN),
        pattern(crate::gwapi_types::PRECISE_HOSTNAME_PATTERN)
    )]
    pub hostname: Option<String>,

    /// Path defines a path rewrite
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<HTTPPathModifier>,
}

/// HTTPPathModifier defines configuration for path modifiers
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPPathModifier {
    /// Type defines the type of path modifier
    #[serde(rename = "type")]
    pub modifier_type: HTTPPathModifierType,

    /// ReplaceFullPath specifies the value with which to replace the full path of a request
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_full_path: Option<String>,

    /// ReplacePrefixMatch specifies the value with which to replace the prefix match of a request
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_prefix_match: Option<String>,
}

/// HTTPPathModifierType defines the type of path redirect or rewrite
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub enum HTTPPathModifierType {
    /// ReplaceFullPath replaces the full path
    ReplaceFullPath,
    /// ReplacePrefixMatch replaces any prefix path
    ReplacePrefixMatch,
}

/// HTTPRequestMirrorFilter defines configuration for the RequestMirror filter.
///
/// Contains ONLY the fields declared by the vendored Gateway API standard CRD
/// (`backendRef`, `fraction`, `percent`). Do NOT add Edgion extension fields
/// here: the vendored CRD is a structural schema, so in K8s conf-center mode
/// the API server silently prunes any undeclared field — the extension becomes
/// a no-op with no admission error. Mirror tuning knobs live in
/// [`MirrorTuning`], carried via `edgion.io/mirror-*` route annotations
/// (HTTPRoute path) or inline plugin config (EdgionPlugins path).
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRequestMirrorFilter {
    /// BackendRef references a resource where mirrored requests are sent
    pub backend_ref: BackendObjectReference,

    /// Fraction represents the fraction of requests that should be mirrored to BackendRef
    /// Support: Extended
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<Fraction>,

    /// Percent of requests to mirror (0-100). Gateway API v1.6.2 field name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<i32>,
}

impl HTTPRequestMirrorFilter {
    /// Validate the standard Gateway API RequestMirror fields independently of
    /// Kubernetes admission. File, etcd, and preserved plugin configuration do
    /// not necessarily pass through the vendored CRD's schema and CEL rules.
    pub fn validation_error(&self) -> Option<String> {
        if self.fraction.is_some() && self.percent.is_some() {
            return Some("only one of fraction or percent may be specified".to_string());
        }

        if let Some(fraction) = &self.fraction {
            let denominator = fraction.denominator.unwrap_or(100);
            if denominator < 1 {
                return Some("fraction.denominator must be at least 1".to_string());
            }
            if fraction.numerator < 0 || fraction.numerator > denominator {
                return Some(format!("fraction.numerator must be between 0 and {denominator}"));
            }
        }

        if let Some(percent) = self.percent {
            if !(0..=100).contains(&percent) {
                return Some("percent must be between 0 and 100".to_string());
            }
        }

        if self.backend_ref.name.is_empty() {
            return Some("backendRef.name must not be empty".to_string());
        }
        if !self.backend_ref.is_service() {
            return Some("backendRef must reference a core Service".to_string());
        }
        match self.backend_ref.port {
            Some(port) if !crate::gwapi_types::is_valid_port(port as i64) => Some(format!(
                "backendRef.port must be between {} and {}",
                crate::gwapi_types::PORT_MIN,
                crate::gwapi_types::PORT_MAX
            )),
            None => Some("backendRef.port is required for a Service reference".to_string()),
            Some(_) => self
                .backend_ref
                .validated_target("default")
                .err()
                .map(|_| "backendRef name and namespace must be valid Kubernetes object names".to_string()),
        }
    }
}

/// Edgion-specific tuning knobs for the RequestMirror filter.
///
/// Kept out of [`HTTPRequestMirrorFilter`] because the vendored Gateway API
/// CRD would silently prune these fields in K8s conf-center mode (structural
/// schema). Sources:
/// - HTTPRoute path: `edgion.io/mirror-*` annotations, pre-parsed in
///   `http_route_preparse.rs` and copied onto every rule
///   (`HTTPRouteRule.parsed_mirror_tuning`).
/// - EdgionPlugins path: inline fields of the `RequestMirror` plugin config
///   (that CRD preserves unknown fields, so no pruning applies).
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MirrorTuning {
    /// Connection timeout for mirror backend in milliseconds.
    #[serde(default = "default_mirror_connect_timeout_ms")]
    pub connect_timeout_ms: u64,

    /// Write/send timeout for mirror backend in milliseconds.
    #[serde(default = "default_mirror_write_timeout_ms")]
    pub write_timeout_ms: u64,

    /// Maximum number of body chunks buffered for mirror when mirror is slower.
    #[serde(default = "default_mirror_max_buffered_chunks")]
    pub max_buffered_chunks: usize,

    /// Maximum bytes retained by one mirror's body stream (default: 1MiB).
    #[serde(default = "default_mirror_max_buffered_bytes")]
    pub max_buffered_bytes: String,

    /// Whether mirror should emit a dedicated access log line.
    #[serde(default = "default_mirror_log_enabled")]
    pub mirror_log: bool,

    /// Maximum concurrent tasks per plugin instance, subject to the process limit.
    #[serde(default = "default_mirror_max_concurrent")]
    pub max_concurrent: usize,

    /// Maximum milliseconds to wait for channel space when the mirror body channel is full,
    /// before abandoning the mirror entirely.
    ///
    /// - `0` (default): immediately abandon — zero impact on main request latency.
    /// - `> 0`: brief back-pressure window — the request body filter will await channel
    ///   drain for at most this many milliseconds before giving up. Recommended range:
    ///   100–500ms. Beyond this the mirror is abandoned and main request continues normally.
    ///
    /// This is a trade-off: a small value gives the mirror backend a chance to catch up
    /// without immediately discarding valuable traffic.
    #[serde(default)]
    pub channel_full_timeout_ms: u64,
}

impl Default for MirrorTuning {
    fn default() -> Self {
        Self {
            connect_timeout_ms: default_mirror_connect_timeout_ms(),
            write_timeout_ms: default_mirror_write_timeout_ms(),
            max_buffered_chunks: default_mirror_max_buffered_chunks(),
            max_buffered_bytes: default_mirror_max_buffered_bytes(),
            mirror_log: default_mirror_log_enabled(),
            max_concurrent: default_mirror_max_concurrent(),
            channel_full_timeout_ms: 0,
        }
    }
}

/// Validated capacities, resolved once before constructing a mirror runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MirrorLimits {
    pub max_concurrent: usize,
    pub max_buffered_chunks: usize,
    pub max_buffered_bytes: usize,
}

impl MirrorTuning {
    pub const MAX_CONCURRENT: usize = 65_536;
    pub const MAX_BUFFERED_CHUNKS: usize = 1024;
    // Tokio's semaphore capacity domain is usize::MAX >> 3. Keep this
    // dependency-independent schema bound safe on 32-bit targets as well.
    pub const MAX_BUFFERED_BYTES: usize = if (usize::MAX >> 3) < 1024 * 1024 * 1024 {
        usize::MAX >> 3
    } else {
        1024 * 1024 * 1024
    };

    fn buffered_bytes(&self) -> Option<usize> {
        let bytes = parse_byte_size(&self.max_buffered_bytes)?;
        let bytes = usize::try_from(bytes).ok()?;
        (1..=Self::MAX_BUFFERED_BYTES).contains(&bytes).then_some(bytes)
    }

    /// Strict validation for the EdgionPlugins configuration entry point.
    pub fn resolve_limits(&self) -> Result<MirrorLimits, String> {
        if !(1..=Self::MAX_CONCURRENT).contains(&self.max_concurrent) {
            return Err(format!("maxConcurrent must be between 1 and {}", Self::MAX_CONCURRENT));
        }
        if !(1..=Self::MAX_BUFFERED_CHUNKS).contains(&self.max_buffered_chunks) {
            return Err(format!(
                "maxBufferedChunks must be between 1 and {}",
                Self::MAX_BUFFERED_CHUNKS
            ));
        }
        let max_buffered_bytes = self.buffered_bytes().ok_or_else(|| {
            format!(
                "maxBufferedBytes must be a byte size between 1B and {}B",
                Self::MAX_BUFFERED_BYTES
            )
        })?;
        Ok(MirrorLimits {
            max_concurrent: self.max_concurrent,
            max_buffered_chunks: self.max_buffered_chunks,
            max_buffered_bytes,
        })
    }

    /// Replace each invalid capacity independently for annotation fallback and
    /// defensive runtime construction. Valid operator choices remain intact.
    pub fn normalize_limits(&mut self) -> MirrorLimits {
        if !(1..=Self::MAX_CONCURRENT).contains(&self.max_concurrent) {
            self.max_concurrent = default_mirror_max_concurrent();
        }
        if !(1..=Self::MAX_BUFFERED_CHUNKS).contains(&self.max_buffered_chunks) {
            self.max_buffered_chunks = default_mirror_max_buffered_chunks();
        }
        let max_buffered_bytes = self.buffered_bytes().unwrap_or_else(|| {
            self.max_buffered_bytes = default_mirror_max_buffered_bytes();
            1024 * 1024
        });
        MirrorLimits {
            max_concurrent: self.max_concurrent,
            max_buffered_chunks: self.max_buffered_chunks,
            max_buffered_bytes,
        }
    }
}

fn default_mirror_max_buffered_bytes() -> String {
    "1MiB".to_string()
}

fn default_mirror_connect_timeout_ms() -> u64 {
    1000
}

fn default_mirror_write_timeout_ms() -> u64 {
    1000
}

fn default_mirror_max_buffered_chunks() -> usize {
    5
}

fn default_mirror_log_enabled() -> bool {
    true
}

fn default_mirror_max_concurrent() -> usize {
    1024
}

/// HTTPCORSFilter defines a Gateway API native CORS filter (GEP-1767).
///
/// The shared validator mirrors the Gateway API v1.5 admission contract for
/// Kubernetes and non-Kubernetes ingestion.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPCORSFilter {
    /// AllowOrigins defines the origins that are permitted to make CORS requests.
    /// Each value must be a valid origin: `scheme://host[:port]` or `*`.
    /// Wildcard `*` is allowed only in the subdomain position (e.g. `https://*.example.com`).
    /// Origins with paths, query strings, fragments, or trailing slashes are rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_origins: Option<Vec<String>>,

    /// AllowCredentials indicates whether the request can include user credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_credentials: Option<bool>,

    /// AllowMethods defines the HTTP methods that are permitted in CORS requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_methods: Option<Vec<String>>,

    /// AllowHeaders defines the HTTP headers that are permitted in CORS requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_headers: Option<Vec<String>>,

    /// ExposeHeaders defines the HTTP headers that browsers are allowed to access.
    /// When allowCredentials is true, use explicit names; the wildcard is unsupported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expose_headers: Option<Vec<String>>,

    /// MaxAge defines the maximum time (in seconds) a preflight response can be cached.
    /// Gateway API defaults an omitted value to 5 and accepts positive int32 values.
    #[serde(default = "default_cors_max_age")]
    pub max_age: u32,
}

const CORS_LIST_MAX_ITEMS: usize = 64;
const CORS_ALLOW_METHODS_MAX_ITEMS: usize = 9;
const CORS_ORIGIN_MAX_LEN: usize = 253;
const CORS_MAX_AGE: u32 = i32::MAX as u32;
const CORS_ALLOWED_METHODS: [&str; 10] = [
    "GET", "HEAD", "POST", "PUT", "DELETE", "CONNECT", "OPTIONS", "TRACE", "PATCH", "*",
];

fn default_cors_max_age() -> u32 {
    5
}

impl HTTPCORSFilter {
    /// Edgion requires explicit exposure names when the policy permits credentials.
    /// Browser credentials mode cannot be inferred from request header presence.
    pub fn credentialed_expose_headers_error(&self) -> Option<&'static str> {
        (self.allow_credentials == Some(true)
            && self
                .expose_headers
                .as_ref()
                .is_some_and(|headers| headers.iter().any(|name| name == "*")))
        .then_some("CORS exposeHeaders must not contain '*' when allowCredentials is true; use explicit header names")
    }

    /// Validate the vendored Gateway API v1.5 CORS admission contract and
    /// Edgion's credentialed exposure support restriction.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if let Some(error) = self.credentialed_expose_headers_error() {
            errors.push(error.to_owned());
        }

        if let Some(origins) = &self.allow_origins {
            Self::validate_set("allowOrigins", origins, CORS_LIST_MAX_ITEMS, true, &mut errors);
            for (i, origin) in origins.iter().enumerate() {
                if let Err(e) = Self::validate_origin(origin) {
                    errors.push(format!("allowOrigins[{}] \"{}\": {}", i, origin, e));
                }
            }
        }
        if let Some(methods) = &self.allow_methods {
            Self::validate_set("allowMethods", methods, CORS_ALLOW_METHODS_MAX_ITEMS, true, &mut errors);
            for (i, method) in methods.iter().enumerate() {
                if !CORS_ALLOWED_METHODS.contains(&method.as_str()) {
                    errors.push(format!(
                        "allowMethods[{i}] \"{method}\": must be a Gateway API v1.5 HTTP method or '*'"
                    ));
                }
            }
        }
        if let Some(headers) = &self.allow_headers {
            Self::validate_set("allowHeaders", headers, CORS_LIST_MAX_ITEMS, true, &mut errors);
            Self::validate_header_names("allowHeaders", headers, &mut errors);
        }
        if let Some(headers) = &self.expose_headers {
            Self::validate_set("exposeHeaders", headers, CORS_LIST_MAX_ITEMS, false, &mut errors);
            Self::validate_header_names("exposeHeaders", headers, &mut errors);
        }
        if !(1..=CORS_MAX_AGE).contains(&self.max_age) {
            errors.push(format!("maxAge must be between 1 and {CORS_MAX_AGE} seconds"));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn validate_set(
        field: &str,
        values: &[String],
        max_items: usize,
        wildcard_exclusive: bool,
        errors: &mut Vec<String>,
    ) {
        if values.len() > max_items {
            errors.push(format!("{field} must contain at most {max_items} entries"));
        }
        let mut seen = HashSet::with_capacity(values.len());
        for (i, value) in values.iter().enumerate() {
            if !seen.insert(value.as_str()) {
                errors.push(format!("{field}[{i}] \"{value}\": duplicate entry"));
            }
        }
        if wildcard_exclusive && values.len() > 1 && values.iter().any(|value| value == "*") {
            errors.push(format!("{field} must not contain '*' alongside other entries"));
        }
    }

    fn validate_header_names(field: &str, headers: &[String], errors: &mut Vec<String>) {
        for (i, header) in headers.iter().enumerate() {
            if header.len() > crate::gwapi_types::HEADER_NAME_MAX_LEN as usize
                || !crate::gwapi_types::is_valid_header_token(header)
            {
                errors.push(format!("{field}[{i}] \"{header}\": must be a valid HTTP header name"));
            }
        }
    }

    /// Validate a single origin value per Gateway API v1.5 rules.
    /// Valid format: `scheme://host[:port]` where host may use `*` only as
    /// a subdomain wildcard prefix (e.g. `*.example.com`).
    fn validate_origin(origin: &str) -> Result<(), String> {
        if origin.is_empty() {
            return Err("must not be empty".to_string());
        }
        if origin.len() > CORS_ORIGIN_MAX_LEN {
            return Err(format!("must be at most {CORS_ORIGIN_MAX_LEN} characters"));
        }
        if origin == "*" {
            return Ok(());
        }

        let Some((scheme, authority)) = origin.split_once("://") else {
            return Err("missing scheme (expected http://host or https://host)".to_string());
        };
        if !matches!(scheme, "http" | "https") {
            return Err("scheme must be 'http' or 'https'".to_string());
        }
        if authority.is_empty() {
            return Err("empty host".to_string());
        }
        if authority.contains('/') {
            return Err("origin must not contain a path component".to_string());
        }
        if authority.contains('?') {
            return Err("origin must not contain a query string".to_string());
        }
        if authority.contains('#') {
            return Err("origin must not contain a fragment".to_string());
        }

        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        };
        if let Some(port) = port {
            if port.is_empty() || port.len() > 5 || !port.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("port must be an integer between 1 and 65535".to_string());
            }
            let port = port
                .parse::<i64>()
                .map_err(|_| "port must be an integer between 1 and 65535".to_string())?;
            if !crate::gwapi_types::is_valid_port(port) {
                return Err("port must be an integer between 1 and 65535".to_string());
            }
        }

        if host == "*" {
            return Ok(());
        }
        let host = host.strip_prefix("*.").unwrap_or(host);
        if host.is_empty() || host.contains('*') {
            return Err("wildcard '*' is valid only as the complete host or a '*.' prefix".to_string());
        }
        if host
            .split('.')
            .any(|label| label.is_empty() || !label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
        {
            return Err("host must contain only non-empty DNS labels".to_string());
        }
        Ok(())
    }
}

/// BackendObjectReference identifies an API object within the namespace of the referrer
/// Used specifically for mirror backend references (without weight/plugins)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackendObjectReference {
    /// Group is the group of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,

    /// Kind is kind of the referent (defaults to "Service")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,

    /// Name is the name of the referent
    pub name: String,

    /// Namespace is the namespace of the backend
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Port specifies the destination port number to use for this resource
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = crate::gwapi_types::PORT_MIN, max = crate::gwapi_types::PORT_MAX))]
    pub port: Option<i32>,

    /// Controller-owned authorization result, transported only to the Gateway.
    /// A denied mirror is skipped without affecting the primary request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub ref_denied: Option<RefDenied>,
}

impl BackendObjectReference {
    /// Mirror endpoint selection supports only core Service references.
    pub fn is_service(&self) -> bool {
        super::refs::targets::is_standard_service(self.group.as_deref(), self.kind.as_deref())
    }
}

/// Fraction represents a fraction of requests to mirror
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Fraction {
    /// Numerator specifies the numerator of the fraction
    pub numerator: i32,

    /// Denominator specifies the denominator of the fraction (defaults to 100)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denominator: Option<i32>,
}

/// LocalObjectReference identifies an API object within the namespace of the referrer
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalObjectReference {
    /// Required group of the referent. An explicit empty group denotes core.
    pub group: String,

    /// Kind is kind of the referent
    pub kind: String,

    /// Name is the name of the referent
    pub name: String,

    /// Controller-resolved namespace of a delegated rule's ExtensionRef.
    ///
    /// LocalObjectReference is operator-local and therefore has no namespace
    /// field. Once a child rule is flattened into a parent HTTPRoute, the
    /// child's local namespace must travel with the derived rule so Controller
    /// dependency tracking and Gateway plugin lookup do not reinterpret it as
    /// parent-local.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_namespace: Option<String>,
}

/// HTTPRouteTimeouts defines timeouts that can be configured for an HTTPRoute
/// Support: Extended
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteTimeouts {
    /// Request specifies the maximum duration for a gateway to respond to an HTTP request
    /// This timeout covers the entire request-response transaction including all retries.
    /// Gateway API v1.4 standard field
    /// Format: Duration (e.g., "10s", "1m", "500ms")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub request: Option<String>,

    /// BackendRequest specifies a timeout for an individual request from the gateway
    /// to a backend. This covers the time from when the request first starts being
    /// sent from the gateway to when the full response has been received from the backend.
    /// Format: Duration (e.g., "10s", "1m", "500ms")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub backend_request: Option<String>,
}

/// HTTPRouteRetry defines retry policy for an HTTPRoute
/// Support: Extended (Experimental)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteRetry {
    /// Attempts specifies the maximum number of times an individual request
    /// from the gateway to a backend should be retried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts: Option<i32>,

    /// Backoff specifies the minimum duration a Gateway should wait between
    /// retry attempts and is represented in Gateway API Duration formatting.
    /// Format: Duration (e.g., "10ms", "1s")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub backoff: Option<String>,

    /// Codes specifies the HTTP response status codes that a Gateway should
    /// retry for. When not specified, retries will be triggered only
    /// on connection errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codes: Option<Vec<i32>>,
}

impl HTTPRouteRetry {
    /// Mirror the v1.6 Experimental retry bounds for non-Kubernetes input.
    pub fn validation_error(&self) -> Option<&'static str> {
        if self.attempts.is_some_and(|attempts| attempts < 1) {
            return Some("retry.attempts must be at least 1");
        }
        if let Some(codes) = &self.codes {
            if codes.iter().any(|code| !(400..=599).contains(code)) {
                return Some("retry.codes must contain only HTTP status codes from 400 to 599");
            }
            let mut seen = [false; 200];
            for code in codes {
                let slot = &mut seen[(code - 400) as usize];
                if *slot {
                    return Some("retry.codes must contain unique status codes");
                }
                *slot = true;
            }
        }
        None
    }
}

#[cfg(test)]
#[path = "../../tests/unit/resources/http_route_test/v16_retry_tests.rs"]
mod v16_retry_tests;

/// SessionPersistence defines the desired state of SessionPersistence
/// Support: Extended (Experimental)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionPersistence {
    /// SessionName defines the name of the persistent session token
    /// which may be reflected in the cookie or the header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,

    /// AbsoluteTimeout defines the absolute timeout of the persistent
    /// session, regardless of activity.
    /// Format: Duration (e.g., "1h", "24h")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub absolute_timeout: Option<String>,

    /// Type defines the type of session persistence such as through
    /// the use a header or cookie.
    /// Defaults to cookie based session persistence.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub persistence_type: Option<SessionPersistenceType>,

    /// CookieConfig provides configuration settings that are specific
    /// to cookie-based session persistence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookie_config: Option<CookieConfig>,

    /// Edgion extension: strict mode.
    /// When true, if the session-bound backend is unavailable, return error (no fallback).
    /// When false (default), fallback to normal load balancing and update the session token.
    /// Similar to Envoy's `strict` field on stateful_session filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

/// SessionPersistenceType specifies the type of session persistence
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub enum SessionPersistenceType {
    /// Cookie uses a cookie for session persistence
    Cookie,
    /// Header uses a header for session persistence
    Header,
}

/// CookieConfig defines configuration for cookie-based session persistence
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CookieConfig {
    /// LifetimeType specifies whether the cookie has a permanent or
    /// session-based lifetime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifetime_type: Option<CookieLifetimeType>,
}

/// CookieLifetimeType specifies the type of cookie lifetime
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub enum CookieLifetimeType {
    /// Permanent cookies persist until deleted by the client
    Permanent,
    /// Session cookies are deleted when the client session ends
    Session,
}

/// Parse a Gateway API GEP-2257 duration (`^([0-9]{1,5}(h|m|s|ms)){1,4}$`) into
/// whole seconds, rounding UP to the nearest second with a floor of 1s so a
/// positive timeout never rounds to 0. Returns `None` for any string outside
/// GEP-2257 (bare integers, decimals, days, long unit names, >5-digit segments,
/// or >4 segments). Segments may be unordered and units may repeat; values
/// accumulate.
pub fn parse_gep2257_duration_secs(s: &str) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    let bytes = s.as_bytes();
    let mut i = 0usize;
    let mut total_ms: u64 = 0;
    let mut segments = 0u32;
    while i < bytes.len() {
        // Digits: 1..=5 of them.
        let digit_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let digit_len = i - digit_start;
        if digit_len == 0 || digit_len > 5 {
            return None;
        }
        let value: u64 = s[digit_start..i].parse().ok()?;
        // Unit: match `ms` before `m`.
        let unit_ms: u64 = if s[i..].starts_with("ms") {
            i += 2;
            1
        } else if s[i..].starts_with('h') {
            i += 1;
            3_600_000
        } else if s[i..].starts_with('m') {
            i += 1;
            60_000
        } else if s[i..].starts_with('s') {
            i += 1;
            1_000
        } else {
            return None;
        };
        total_ms = total_ms.checked_add(value.checked_mul(unit_ms)?)?;
        segments += 1;
        if segments > 4 {
            return None;
        }
    }
    // ceil to seconds, floor 1.
    let secs = total_ms.div_ceil(1_000);
    Some(secs.max(1))
}

impl SessionPersistence {
    /// Validate the fields this gateway enforces at config-load time, mirroring
    /// the upstream Gateway API CRD's admission rules for the modes (FileSystem/
    /// Etcd) that have no CRD/CEL admission. Returns `Err(reason)` with an
    /// operator-readable message; a failing rule is dropped via
    /// `invalid_rule_indices` (never silently downgraded).
    pub fn validate(&self) -> Result<(), String> {
        if let Some(t) = self.absolute_timeout.as_deref() {
            if parse_gep2257_duration_secs(t).is_none() {
                return Err(format!(
                    "absoluteTimeout '{t}' is not a valid Gateway API duration (GEP-2257: e.g. 1h, 30m, 1h30m, 500ms)"
                ));
            }
        }
        let is_permanent = self
            .cookie_config
            .as_ref()
            .and_then(|c| c.lifetime_type.as_ref())
            .is_some_and(|lt| *lt == CookieLifetimeType::Permanent);
        if is_permanent && self.absolute_timeout.is_none() {
            return Err("cookieConfig.lifetimeType 'Permanent' requires absoluteTimeout".to_string());
        }
        Ok(())
    }
}

// ============================================
// Pre-parsed Route Retry
// ============================================

/// Pre-parsed retry configuration from HTTPRouteRule.retry
/// Stored in HTTPRouteRule for zero-cost runtime access
#[derive(Debug, Clone)]
pub struct ParsedRouteRetry {
    /// Max retry attempts (from retry.attempts)
    pub attempts: Option<u32>,
    /// Base backoff duration (from retry.backoff, parsed from string)
    pub backoff: Option<std::time::Duration>,
    /// HTTP status codes that trigger retry (from retry.codes, validated to 400-599)
    pub codes: Option<Vec<u16>>,
}

// ============================================
// Pre-parsed Route Timeouts
// ============================================

use std::time::Duration;

/// Three-state timeout setting per Gateway API spec.
/// - `Unset`: field not specified → fall back to global default
/// - `Disabled`: explicitly set to `"0s"` → no route-configured timeout
///   (transport-level liveness floors may still apply)
/// - `Duration`: explicitly set to a positive value
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteTimeout {
    Unset,
    Disabled,
    Duration(Duration),
}

impl RouteTimeout {
    /// Returns the duration if explicitly set to a positive value, otherwise None.
    pub fn as_duration(&self) -> Option<Duration> {
        match self {
            RouteTimeout::Duration(d) => Some(*d),
            _ => None,
        }
    }

    pub fn parse(s: &str, field_name: &str) -> Self {
        use crate::duration::parse_duration;
        match parse_duration(s) {
            Ok(d) if d.is_zero() => RouteTimeout::Disabled,
            Ok(d) => RouteTimeout::Duration(d),
            Err(e) => {
                tracing::warn!("Invalid {} timeout '{}': {}", field_name, s, e);
                RouteTimeout::Unset
            }
        }
    }
}

#[derive(Clone)]
pub struct ParsedRouteTimeouts {
    /// Parsed request timeout (end-to-end including retries)
    /// Gateway API v1.4 standard field
    pub request_timeout: RouteTimeout,

    /// Parsed backend request timeout
    /// Gateway API v1.4 standard field
    pub backend_request_timeout: RouteTimeout,
}

impl ParsedRouteTimeouts {
    /// Parse route timeouts from HTTPRouteTimeouts
    /// Returns None if no timeouts are configured
    pub fn from_config(config: &HTTPRouteTimeouts) -> Option<Self> {
        let request_timeout = config
            .request
            .as_ref()
            .map(|s| RouteTimeout::parse(s, "request"))
            .unwrap_or(RouteTimeout::Unset);

        let backend_request_timeout = config
            .backend_request
            .as_ref()
            .map(|s| RouteTimeout::parse(s, "backend_request"))
            .unwrap_or(RouteTimeout::Unset);

        // Clamp: backendRequest must be <= request (Gateway API spec)
        let backend_request_timeout = match (backend_request_timeout.as_duration(), request_timeout.as_duration()) {
            (Some(br), Some(req)) if br > req => {
                tracing::warn!(
                    "backendRequest timeout ({:?}) exceeds request timeout ({:?}), clamping to request timeout",
                    br,
                    req
                );
                RouteTimeout::Duration(req)
            }
            _ => backend_request_timeout,
        };

        // Only create if at least one timeout is explicitly configured
        if matches!(request_timeout, RouteTimeout::Unset) && matches!(backend_request_timeout, RouteTimeout::Unset) {
            None
        } else {
            Some(Self {
                request_timeout,
                backend_request_timeout,
            })
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteStatus {
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

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RouteParentStatus {
    /// ParentRef references the parent that this status corresponds to.
    pub parent_ref: ParentReference,

    /// ControllerName is the name of the controller that manages this Route.
    pub controller_name: String,

    /// Conditions describe the current conditions of this route.
    pub conditions: Vec<Condition>,
}

#[cfg(test)]
#[path = "../../tests/unit/resources/http_route_test.rs"]
mod tests;
