//! EdgionBackendTrafficPolicy — per-destination traffic policy (LB algorithm,
//! active health check, outlier detection, panic threshold) attached to
//! Services or EdgionAIService resources via targetRefs. Design:
//! docs/superpowers/specs/2026-07-10-edgion-backend-traffic-policy-design.md

use super::circuit_breaker::CircuitBreakerConfig;
use super::common::PolicyAncestorStatus;
use super::health_check::ServiceHealthCheck;
use super::lb_policy::{ConsistentHashOn, ParsedLBPolicy};
use super::outlier_detection::OutlierDetectionConfig;
use super::upstream_authority::{validate_upstream_authority, UpstreamAuthorityConfig};
use kube::core::ObjectMeta;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "edgion.io",
    version = "v1",
    kind = "EdgionBackendTrafficPolicy",
    plural = "edgionbackendtrafficpolicies",
    shortname = "ebtp",
    namespaced,
    status = "EdgionBackendTrafficPolicyStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct EdgionBackendTrafficPolicySpec {
    /// Homogeneous Service or EdgionAIService targets in the policy namespace.
    #[schemars(length(min = 1, max = 16))]
    pub target_refs: Vec<PolicyTargetRef>,
    /// Load-balancing algorithm and panic threshold. Service targets only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_balancer: Option<LoadBalancerConfig>,
    /// Active health check (probe) configuration. Service targets only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_check: Option<ServiceHealthCheck>,
    /// Outlier detection (passive ejection) configuration for a Service
    /// endpoint or AI credential target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outlier_detection: Option<OutlierDetectionConfig>,
    /// Dynamic upstream authority (Host + SNI) within a wildcard pattern the
    /// Service backend declares. Service targets only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_authority: Option<UpstreamAuthorityConfig>,
    /// Retry budget: cap retry volume as a percentage of request volume to the
    /// target backend over a sliding window. Absent = today's behaviour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_constraint: Option<RetryConstraintConfig>,
    /// Circuit breaker: hard cap on concurrent in-flight requests per Service
    /// endpoint or per EdgionAIService. Absent = no concurrency cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub circuit_breaker: Option<CircuitBreakerConfig>,
    /// Per-target connection tuning override. Currently only connectTimeout.
    /// Absent = use the global defaults. HTTP upstream only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<ConnectionOverride>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RetryConstraintConfig {
    /// Percentage budget. Defaults to {percent: 20, interval: "10s"} to match
    /// upstream, so `retryConstraint: {}` is a valid 20%/10s budget.
    #[serde(default)]
    pub budget: BudgetDetails,
    /// Floor so low-traffic backends can still retry when the percentage
    /// budget rounds to ~0. Defaults to {count: 10, interval: "1s"} to match
    /// upstream. The floor is ALWAYS present (matching upstream, which has no
    /// null option); the CRD does NOT declare `nullable`, so the API server
    /// rejects `minRetryRate: null`. Kept as `Option` only so the resolver can
    /// represent the (never-serialized-from-a-CR) internal case; operators
    /// tune it via count/interval, they do not disable it.
    #[serde(default = "default_min_retry_rate")]
    pub min_retry_rate: Option<RateThreshold>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BudgetDetails {
    /// Max retries as a percentage of total attempts, 0-100. Default 20.
    #[serde(default = "default_budget_percent")]
    pub percent: u8,
    /// Sliding window over which the percentage is computed (GEP-2257).
    /// Constrained to [1s, 1h] (see validation), matching upstream CEL.
    #[serde(default = "default_budget_interval")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub interval: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RateThreshold {
    /// Retries allowed per `interval` regardless of the percentage budget.
    /// 1..=1_000_000 (see validation), matching upstream.
    pub count: u32,
    /// Window over which `count` is measured (GEP-2257), independent of
    /// `budget.interval`. Constrained to (0s, 1h] (see validation).
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub interval: String,
}

impl Default for BudgetDetails {
    fn default() -> Self {
        Self {
            percent: 20,
            interval: "10s".into(),
        }
    }
}
fn default_budget_percent() -> u8 {
    20
}
fn default_budget_interval() -> String {
    "10s".into()
}
fn default_min_retry_rate() -> Option<RateThreshold> {
    Some(RateThreshold {
        count: 10,
        interval: "1s".into(),
    })
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionOverride {
    /// Per-backend gateway->backend connection-establishment deadline (GEP-2257).
    /// Overrides EdgionGatewayConfig.spec.httpTimeout.backend.defaultConnectTimeout
    /// for the targeted Services. Absent = use the global default. HTTP upstream
    /// connections only:
    /// stream (TCPRoute/TLSRoute) backends use tcpTimeout.connectTimeout and
    /// ignore this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub connect_timeout: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PolicyTargetRef {
    /// Group of the referent. Empty string (default) = core API group.
    #[serde(default)]
    pub group: String,
    /// Kind of the referent: "Service" or "EdgionAIService".
    pub kind: String,
    /// Name of the referent (same namespace as the policy).
    pub name: String,
}

/// Canonical kind of an EdgionBackendTrafficPolicy target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PolicyTargetKind {
    Service,
    EdgionAIService,
}

impl PolicyTargetKind {
    pub const fn group(self) -> &'static str {
        match self {
            Self::Service => "",
            Self::EdgionAIService => "edgion.io",
        }
    }

    pub const fn kind(self) -> &'static str {
        match self {
            Self::Service => "Service",
            Self::EdgionAIService => "EdgionAIService",
        }
    }
}

/// Typed, canonical identity used for policy arbitration and dependency keys.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PolicyTargetKey {
    pub target_kind: PolicyTargetKind,
    pub namespace: String,
    pub name: String,
}

impl PolicyTargetKey {
    pub fn group(&self) -> &'static str {
        self.target_kind.group()
    }

    pub fn kind(&self) -> &'static str {
        self.target_kind.kind()
    }
}

impl fmt::Display for PolicyTargetKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}/{}", self.group(), self.kind(), self.namespace, self.name)
    }
}

impl PolicyTargetRef {
    /// Parse the accepted group/kind pairs into their canonical typed kind.
    pub fn target_kind(&self) -> Result<PolicyTargetKind, String> {
        match (self.group.as_str(), self.kind.as_str()) {
            ("" | "core", "Service") => Ok(PolicyTargetKind::Service),
            ("edgion.io", "EdgionAIService") => Ok(PolicyTargetKind::EdgionAIService),
            _ => Err(format!(
                "targetRef {}/{} {}: supported targets are core-group Service or edgion.io/EdgionAIService",
                self.group, self.kind, self.name
            )),
        }
    }

    /// Resolve this same-namespace ref into its typed canonical identity.
    pub fn canonical_key(&self, namespace: &str) -> Result<PolicyTargetKey, String> {
        Ok(PolicyTargetKey {
            target_kind: self.target_kind()?,
            namespace: namespace.to_string(),
            name: self.name.clone(),
        })
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LoadBalancerConfig {
    /// Algorithm. RoundRobin is the engine default.
    #[serde(rename = "type")]
    pub r#type: LoadBalancerType,
    /// Required iff type == ConsistentHash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consistent_hash: Option<ConsistentHashConfig>,
    /// Percent 0-100. Overrides the global
    /// EdgionGatewayConfig.spec.loadBalancing.panicThreshold for the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panic_threshold: Option<u8>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub enum LoadBalancerType {
    RoundRobin,
    LeastConn,
    Ewma,
    ConsistentHash,
}

/// NOTE: the field is `hashOn`, not `on` — unquoted `on` is a YAML 1.1
/// boolean and kubectl's YAML converter would turn the key into "true".
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConsistentHashConfig {
    pub hash_on: HashOn,
    /// Header name / cookie name / query parameter name to hash on. Absent for
    /// `SourceIp`, which needs no value name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum HashOn {
    Header,
    Cookie,
    QueryParam,
    SourceIp,
}

/// GEP-2648 per-ancestor policy status. The ancestor of this direct policy
/// is the Gateway whose routes reach a target; the same policy can
/// be Accepted on one Gateway and Conflicted on another, which a flat
/// condition list cannot express.
#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EdgionBackendTrafficPolicyStatus {
    /// One entry per ancestor Gateway, stamped with this controller's name.
    /// Empty when no route references any target. Capped at 16.
    ///
    /// Always serialized, even when empty: K8s status persistence uses JSON
    /// Merge Patch, and merge-patching an empty status object `{}` cannot
    /// clear a previously-persisted non-empty list — only an explicit
    /// `"ancestors": []` replaces it. Do not add skip_serializing_if here.
    #[serde(default)]
    pub ancestors: Vec<PolicyAncestorStatus>,
}

impl EdgionBackendTrafficPolicySpec {
    /// Return canonical typed identities for every target in the policy
    /// namespace. Callers should use this only after validation succeeds.
    pub fn canonical_target_keys(&self, namespace: &str) -> Result<Vec<PolicyTargetKey>, String> {
        self.target_refs
            .iter()
            .map(|target| target.canonical_key(namespace))
            .collect()
    }

    pub fn get_validation_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.target_refs.is_empty() {
            errors.push("targetRefs must not be empty".to_string());
        } else if self.target_refs.len() > 16 {
            errors.push("targetRefs must contain at most 16 entries".to_string());
        }

        let mut target_kinds = HashSet::new();
        let mut targets = HashSet::new();
        for tr in &self.target_refs {
            match tr.target_kind() {
                Ok(target_kind) => {
                    target_kinds.insert(target_kind);
                    if !targets.insert((target_kind, tr.name.as_str())) {
                        errors.push(format!(
                            "targetRefs must not contain duplicate target {}/{} {}",
                            target_kind.group(),
                            target_kind.kind(),
                            tr.name
                        ));
                    }
                }
                Err(error) => errors.push(error),
            }
            if tr.name.is_empty() {
                errors.push("targetRef name must not be empty".to_string());
            }
        }
        if target_kinds.len() > 1 {
            errors.push("targetRefs must all have the same target kind".to_string());
        }

        let targets_ai_service = target_kinds.len() == 1 && target_kinds.contains(&PolicyTargetKind::EdgionAIService);
        if targets_ai_service {
            if self.load_balancer.is_some() {
                errors.push("loadBalancer is not supported for EdgionAIService targets".to_string());
            }
            if self.health_check.is_some() {
                errors.push("healthCheck is not supported for EdgionAIService targets".to_string());
            }
            if self.upstream_authority.is_some() {
                errors.push("upstreamAuthority is not supported for EdgionAIService targets".to_string());
            }
        }
        if let Some(lb) = &self.load_balancer {
            match (&lb.r#type, &lb.consistent_hash) {
                (LoadBalancerType::ConsistentHash, None) => {
                    errors.push("loadBalancer.consistentHash is required when type is ConsistentHash".to_string());
                }
                (LoadBalancerType::ConsistentHash, Some(ch)) => match ch.hash_on {
                    HashOn::SourceIp => {
                        if ch.key.is_some() {
                            errors.push(
                                "loadBalancer.consistentHash.key must be absent when hashOn is sourceIp".to_string(),
                            );
                        }
                    }
                    HashOn::Header | HashOn::Cookie | HashOn::QueryParam => {
                        if ch.key.as_deref().unwrap_or_default().is_empty() {
                            errors.push("loadBalancer.consistentHash.key must not be empty".to_string());
                        }
                    }
                },
                (t, Some(_)) if *t != LoadBalancerType::ConsistentHash => {
                    errors.push("loadBalancer.consistentHash is only valid when type is ConsistentHash".to_string());
                }
                _ => {}
            }
            if let Some(pt) = lb.panic_threshold {
                if pt > 100 {
                    errors.push("loadBalancer.panicThreshold must be 0-100".to_string());
                }
            }
        }
        if let Some(hc) = &self.health_check {
            if let Some(err) = hc.get_validation_error() {
                errors.push(format!("healthCheck: {err}"));
            }
        }
        if let Some(od) = &self.outlier_detection {
            if let Some(err) = od.get_validation_error() {
                errors.push(format!("outlierDetection: {err}"));
            }
        }
        if let Some(cb) = &self.circuit_breaker {
            if let Some(err) = cb.get_validation_error() {
                errors.push(format!("circuitBreaker: {err}"));
            }
        }
        if let Some(ua) = &self.upstream_authority {
            let has_active_hc = self.health_check.as_ref().map(|h| h.active.is_some()).unwrap_or(false);
            errors.extend(validate_upstream_authority(ua, has_active_hc));
        }
        if let Some(rc) = &self.retry_constraint {
            if rc.budget.percent > 100 {
                errors.push("retryConstraint.budget.percent must be 0-100".to_string());
            }
            match crate::duration::parse_duration(&rc.budget.interval) {
                Err(e) => errors.push(format!("retryConstraint.budget.interval: {e}")),
                Ok(d) => {
                    if d < std::time::Duration::from_secs(1) || d > std::time::Duration::from_secs(3600) {
                        errors.push("retryConstraint.budget.interval must be in [1s, 1h]".to_string());
                    }
                }
            }
            if let Some(f) = &rc.min_retry_rate {
                if !(1..=1_000_000).contains(&f.count) {
                    errors.push("retryConstraint.minRetryRate.count must be in 1..=1000000".to_string());
                }
                match crate::duration::parse_duration(&f.interval) {
                    Err(e) => errors.push(format!("retryConstraint.minRetryRate.interval: {e}")),
                    Ok(d) => {
                        if d.is_zero() || d > std::time::Duration::from_secs(3600) {
                            errors.push("retryConstraint.minRetryRate.interval must be in (0s, 1h]".to_string());
                        }
                    }
                }
            }
        }
        if let Some(conn) = &self.connection {
            if let Some(ct) = &conn.connect_timeout {
                match crate::duration::parse_duration(ct) {
                    Err(e) => errors.push(format!("connection.connectTimeout: {e}")),
                    Ok(d) => {
                        if d.is_zero() || d > std::time::Duration::from_secs(3600) {
                            errors.push("connection.connectTimeout must be in (0s, 1h]".to_string());
                        }
                    }
                }
            }
        }
        errors
    }
}

impl LoadBalancerConfig {
    pub fn to_parsed(&self) -> Option<ParsedLBPolicy> {
        match self.r#type {
            LoadBalancerType::RoundRobin => None,
            LoadBalancerType::LeastConn => Some(ParsedLBPolicy::LeastConn),
            LoadBalancerType::Ewma => Some(ParsedLBPolicy::Ewma),
            LoadBalancerType::ConsistentHash => {
                let ch = self.consistent_hash.as_ref()?;
                let on = match ch.hash_on {
                    HashOn::Header => ConsistentHashOn::Header(ch.key.clone().unwrap_or_default()),
                    HashOn::Cookie => ConsistentHashOn::Cookie(ch.key.clone().unwrap_or_default()),
                    HashOn::QueryParam => ConsistentHashOn::Arg(ch.key.clone().unwrap_or_default()),
                    HashOn::SourceIp => ConsistentHashOn::SourceIp,
                };
                Some(ParsedLBPolicy::ConsistentHash(on))
            }
        }
    }
}

/// Oldest-wins conflict ordering: lower key wins. Absent creationTimestamp
/// (file-system conf center) loses to any timestamped policy; final tie-break
/// is the resource name.
pub fn precedence_key(meta: &ObjectMeta) -> (i64, String) {
    let ts = meta
        .creation_timestamp
        .as_ref()
        .map(|t| t.0.timestamp())
        .unwrap_or(i64::MAX);
    (ts, meta.name.clone().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_serializes_gep2648_ancestor_shape() {
        use crate::resources::common::{ParentReference, PolicyAncestorStatus};
        let status = EdgionBackendTrafficPolicyStatus {
            ancestors: vec![PolicyAncestorStatus {
                ancestor_ref: ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some("ns".to_string()),
                    name: "gw".to_string(),
                    section_name: None,
                    port: None,
                },
                controller_name: "example.io/controller".to_string(),
                conditions: vec![],
            }],
        };
        let v = serde_json::to_value(&status).unwrap();
        assert_eq!(v["ancestors"][0]["ancestorRef"]["name"], "gw");
        assert_eq!(v["ancestors"][0]["controllerName"], "example.io/controller");
        // Empty ancestors still serialize an explicit `"ancestors": []`, never
        // an empty object: K8s status persistence uses JSON Merge Patch, and
        // merge-patching `{"status": {}}` cannot clear a previously-persisted
        // non-empty ancestors list — only an explicit empty array replaces it.
        let empty = serde_json::to_value(EdgionBackendTrafficPolicyStatus::default()).unwrap();
        assert_eq!(empty, serde_json::json!({"ancestors": []}));
    }

    #[test]
    fn backend_tls_policy_status_empty_ancestors_serializes_explicit_array() {
        use crate::resources::backend_tls_policy::BackendTLSPolicyStatus;
        // Same merge-patch tombstone concern as EdgionBackendTrafficPolicyStatus
        // above: an empty ancestors list must serialize as `"ancestors": []`,
        // not be omitted, or a JSON Merge Patch can never clear a
        // previously-persisted non-empty list.
        let empty = serde_json::to_value(BackendTLSPolicyStatus::default()).unwrap();
        assert_eq!(empty, serde_json::json!({"ancestors": []}));
    }

    fn minimal_spec_yaml() -> &'static str {
        r#"
targetRefs:
  - kind: Service
    name: payments
loadBalancer:
  type: ConsistentHash
  consistentHash:
    hashOn: header
    key: x-user-id
  panicThreshold: 30
outlierDetection:
  consecutiveErrors: 3
"#
    }

    #[test]
    fn deserialize_full_spec() {
        let spec: EdgionBackendTrafficPolicySpec = serde_yaml::from_str(minimal_spec_yaml()).expect("deserialize");
        assert_eq!(spec.target_refs[0].name, "payments");
        let lb = spec.load_balancer.as_ref().expect("lb");
        assert_eq!(lb.r#type, LoadBalancerType::ConsistentHash);
        let ch = lb.consistent_hash.as_ref().expect("ch");
        assert_eq!(ch.hash_on, HashOn::Header);
        assert_eq!(ch.key.as_deref(), Some("x-user-id"));
        assert_eq!(lb.panic_threshold, Some(30));
        assert!(spec.get_validation_errors().is_empty());
    }

    #[test]
    fn canonical_target_keys_preserve_target_type() {
        let service = PolicyTargetRef {
            group: "core".to_string(),
            kind: "Service".to_string(),
            name: "shared".to_string(),
        }
        .canonical_key("app")
        .expect("core Service target");
        let ai_service = PolicyTargetRef {
            group: "edgion.io".to_string(),
            kind: "EdgionAIService".to_string(),
            name: "shared".to_string(),
        }
        .canonical_key("app")
        .expect("EdgionAIService target");

        assert_eq!(service.target_kind, PolicyTargetKind::Service);
        assert_eq!(service.to_string(), "/Service/app/shared");
        assert_eq!(ai_service.target_kind, PolicyTargetKind::EdgionAIService);
        assert_eq!(ai_service.to_string(), "edgion.io/EdgionAIService/app/shared");
        assert_ne!(service, ai_service);
    }

    #[test]
    fn target_refs_reject_bounds_duplicates_and_mixed_kinds() {
        let empty: EdgionBackendTrafficPolicySpec = serde_yaml::from_str("targetRefs: []").unwrap();
        assert!(empty
            .get_validation_errors()
            .iter()
            .any(|error| error.contains("must not be empty")));

        let too_many_refs = (0..17).map(|index| format!("  - kind: Service\n    name: svc-{index}"));
        let too_many: EdgionBackendTrafficPolicySpec = serde_yaml::from_str(&format!(
            "targetRefs:\n{}",
            too_many_refs.collect::<Vec<_>>().join("\n")
        ))
        .unwrap();
        assert!(too_many
            .get_validation_errors()
            .iter()
            .any(|error| error.contains("at most 16")));

        let duplicate: EdgionBackendTrafficPolicySpec = serde_yaml::from_str(
            "targetRefs:\n  - group: core\n    kind: Service\n    name: shared\n  - group: \"\"\n    kind: Service\n    name: shared",
        )
        .unwrap();
        assert!(duplicate
            .get_validation_errors()
            .iter()
            .any(|error| error.contains("duplicate target")));

        let mixed: EdgionBackendTrafficPolicySpec = serde_yaml::from_str(
            "targetRefs:\n  - kind: Service\n    name: backend\n  - group: edgion.io\n    kind: EdgionAIService\n    name: provider",
        )
        .unwrap();
        assert!(mixed
            .get_validation_errors()
            .iter()
            .any(|error| error.contains("same target kind")));
    }

    #[test]
    fn ai_targets_enforce_policy_field_matrix() {
        let allowed: EdgionBackendTrafficPolicySpec = serde_yaml::from_str(
            "targetRefs:\n  - group: edgion.io\n    kind: EdgionAIService\n    name: provider\nconnection:\n  connectTimeout: 2s\nretryConstraint: {}\ncircuitBreaker:\n  maxParallelRequests: 20\noutlierDetection:\n  consecutiveErrors: 5",
        )
        .unwrap();
        assert!(allowed.get_validation_errors().is_empty());

        for forbidden in [
            "loadBalancer:\n  type: RoundRobin",
            "healthCheck:\n  active: {}",
            "upstreamAuthority:\n  pattern: '*.example.com'\n  template: '${ctx:tenant}.example.com'",
        ] {
            let yaml = format!(
                "targetRefs:\n  - group: edgion.io\n    kind: EdgionAIService\n    name: provider\n{forbidden}"
            );
            let spec: EdgionBackendTrafficPolicySpec = serde_yaml::from_str(&yaml).unwrap();
            assert!(
                spec.get_validation_errors()
                    .iter()
                    .any(|error| error.contains("not supported for EdgionAIService targets")),
                "field must be rejected: {forbidden}"
            );
        }
    }

    #[test]
    fn to_parsed_mapping() {
        use crate::resources::lb_policy::{ConsistentHashOn, ParsedLBPolicy};
        let mk = |t, ch| LoadBalancerConfig {
            r#type: t,
            consistent_hash: ch,
            panic_threshold: None,
        };
        assert_eq!(mk(LoadBalancerType::RoundRobin, None).to_parsed(), None);
        assert_eq!(
            mk(LoadBalancerType::LeastConn, None).to_parsed(),
            Some(ParsedLBPolicy::LeastConn)
        );
        assert_eq!(mk(LoadBalancerType::Ewma, None).to_parsed(), Some(ParsedLBPolicy::Ewma));
        let ch = ConsistentHashConfig {
            hash_on: HashOn::QueryParam,
            key: Some("uid".into()),
        };
        assert_eq!(
            mk(LoadBalancerType::ConsistentHash, Some(ch)).to_parsed(),
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Arg("uid".into())))
        );
        // Header hashOn maps to ConsistentHashOn::Header.
        let ch_header = ConsistentHashConfig {
            hash_on: HashOn::Header,
            key: Some("x-user-id".into()),
        };
        assert_eq!(
            mk(LoadBalancerType::ConsistentHash, Some(ch_header)).to_parsed(),
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Header(
                "x-user-id".into()
            )))
        );
        // Cookie hashOn maps to ConsistentHashOn::Cookie.
        let ch_cookie = ConsistentHashConfig {
            hash_on: HashOn::Cookie,
            key: Some("session-id".into()),
        };
        assert_eq!(
            mk(LoadBalancerType::ConsistentHash, Some(ch_cookie)).to_parsed(),
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Cookie(
                "session-id".into()
            )))
        );
        // SourceIp hashOn maps to the no-argument ConsistentHashOn::SourceIp.
        let ch_source_ip = ConsistentHashConfig {
            hash_on: HashOn::SourceIp,
            key: None,
        };
        assert_eq!(
            mk(LoadBalancerType::ConsistentHash, Some(ch_source_ip)).to_parsed(),
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::SourceIp))
        );
    }

    #[test]
    fn source_ip_hash_accepts_absent_key() {
        let spec: EdgionBackendTrafficPolicySpec = serde_yaml::from_str(
            "targetRefs:\n  - kind: Service\n    name: x\nloadBalancer:\n  type: ConsistentHash\n  consistentHash:\n    hashOn: sourceIp",
        )
        .unwrap();
        assert!(
            spec.get_validation_errors().is_empty(),
            "sourceIp with no key must be accepted: {:?}",
            spec.get_validation_errors()
        );
        // And it deserializes to the keyless config.
        let ch = spec.load_balancer.unwrap().consistent_hash.unwrap();
        assert_eq!(ch.hash_on, HashOn::SourceIp);
        assert_eq!(ch.key, None);
    }

    #[test]
    fn source_ip_hash_rejects_present_key() {
        let spec: EdgionBackendTrafficPolicySpec = serde_yaml::from_str(
            "targetRefs:\n  - kind: Service\n    name: x\nloadBalancer:\n  type: ConsistentHash\n  consistentHash:\n    hashOn: sourceIp\n    key: x-user-id",
        )
        .unwrap();
        assert!(
            spec.get_validation_errors()
                .iter()
                .any(|e| e.contains("key must be absent when hashOn is sourceIp")),
            "sourceIp with a key must be rejected"
        );
    }

    #[test]
    fn validation_rejects_bad_specs() {
        // empty targetRefs
        let empty: EdgionBackendTrafficPolicySpec = serde_yaml::from_str("targetRefs: []").unwrap();
        assert!(!empty.get_validation_errors().is_empty());
        // non-Service kind
        let bad_kind: EdgionBackendTrafficPolicySpec =
            serde_yaml::from_str("targetRefs:\n  - kind: Deployment\n    name: x").unwrap();
        assert!(!bad_kind.get_validation_errors().is_empty());
        // ConsistentHash without consistentHash section
        let no_ch: EdgionBackendTrafficPolicySpec =
            serde_yaml::from_str("targetRefs:\n  - kind: Service\n    name: x\nloadBalancer:\n  type: ConsistentHash")
                .unwrap();
        assert!(!no_ch.get_validation_errors().is_empty());
        // panicThreshold > 100
        let big_pt: EdgionBackendTrafficPolicySpec = serde_yaml::from_str(
            "targetRefs:\n  - kind: Service\n    name: x\nloadBalancer:\n  type: RoundRobin\n  panicThreshold: 101",
        )
        .unwrap();
        assert!(!big_pt.get_validation_errors().is_empty());
        // empty targetRef name
        let empty_name: EdgionBackendTrafficPolicySpec =
            serde_yaml::from_str("targetRefs:\n  - kind: Service\n    name: \"\"").unwrap();
        assert!(
            empty_name
                .get_validation_errors()
                .iter()
                .any(|e| e.contains("targetRef name must not be empty")),
            "empty targetRef name must be rejected"
        );
        // consistentHash present with a non-ConsistentHash type
        let ch_wrong_type: EdgionBackendTrafficPolicySpec = serde_yaml::from_str(
            "targetRefs:\n  - kind: Service\n    name: x\nloadBalancer:\n  type: RoundRobin\n  consistentHash:\n    hashOn: header\n    key: x-user-id",
        )
        .unwrap();
        assert!(
            ch_wrong_type
                .get_validation_errors()
                .iter()
                .any(|e| e.contains("consistentHash is only valid when type is ConsistentHash")),
            "consistentHash with a non-ConsistentHash type must be rejected"
        );
        // consistentHash present but key is empty
        let ch_empty_key: EdgionBackendTrafficPolicySpec = serde_yaml::from_str(
            "targetRefs:\n  - kind: Service\n    name: x\nloadBalancer:\n  type: ConsistentHash\n  consistentHash:\n    hashOn: header\n    key: \"\"",
        )
        .unwrap();
        assert!(
            ch_empty_key
                .get_validation_errors()
                .iter()
                .any(|e| e.contains("consistentHash.key must not be empty")),
            "empty consistentHash.key must be rejected"
        );
        // invalid nested healthCheck — the `healthCheck: {err}` prefix is exercised
        let bad_hc: EdgionBackendTrafficPolicySpec = serde_yaml::from_str(
            "targetRefs:\n  - kind: Service\n    name: x\nhealthCheck:\n  active:\n    healthyThreshold: 0",
        )
        .unwrap();
        assert!(
            bad_hc
                .get_validation_errors()
                .iter()
                .any(|e| e.starts_with("healthCheck:")),
            "invalid nested healthCheck must surface with a healthCheck: prefix"
        );
        // invalid nested outlierDetection — the `outlierDetection: {err}` prefix is exercised
        let bad_od: EdgionBackendTrafficPolicySpec = serde_yaml::from_str(
            "targetRefs:\n  - kind: Service\n    name: x\noutlierDetection:\n  consecutiveErrors: 0",
        )
        .unwrap();
        assert!(
            bad_od
                .get_validation_errors()
                .iter()
                .any(|e| e.starts_with("outlierDetection:")),
            "invalid nested outlierDetection must surface with an outlierDetection: prefix"
        );
        // invalid nested circuitBreaker — the `circuitBreaker: {err}` prefix is exercised
        let bad_cb: EdgionBackendTrafficPolicySpec = serde_yaml::from_str(
            "targetRefs:\n  - kind: Service\n    name: x\ncircuitBreaker:\n  maxParallelRequests: 0",
        )
        .unwrap();
        assert!(
            bad_cb
                .get_validation_errors()
                .iter()
                .any(|e| e.starts_with("circuitBreaker:")),
            "invalid nested circuitBreaker must surface with a circuitBreaker: prefix"
        );
    }

    #[test]
    fn precedence_key_orders_oldest_then_name() {
        use kube::core::ObjectMeta;
        let mk = |name: &str, ts: Option<&str>| ObjectMeta {
            name: Some(name.into()),
            creation_timestamp: ts
                .map(|t| k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(t.parse().expect("rfc3339"))),
            ..Default::default()
        };
        let older = precedence_key(&mk("b", Some("2026-01-01T00:00:00Z")));
        let newer = precedence_key(&mk("a", Some("2026-06-01T00:00:00Z")));
        let no_ts = precedence_key(&mk("a", None));
        assert!(older < newer, "older timestamp wins regardless of name");
        assert!(newer < no_ts, "any timestamp beats absent timestamp");
        // Equal timestamps: the remaining tie breaks by name lexicographically.
        let tie_a = precedence_key(&mk("a", Some("2026-01-01T00:00:00Z")));
        let tie_b = precedence_key(&mk("b", Some("2026-01-01T00:00:00Z")));
        assert!(tie_a < tie_b, "equal timestamps break ties by name (a < b)");
    }

    #[test]
    fn upstream_authority_validation_flows_through_spec() {
        use crate::resources::health_check::{ActiveHealthCheckConfig, ServiceHealthCheck};
        use crate::resources::upstream_authority::UpstreamAuthorityConfig;

        let base = |ua: Option<UpstreamAuthorityConfig>, hc: bool| EdgionBackendTrafficPolicySpec {
            target_refs: vec![PolicyTargetRef {
                group: String::new(),
                kind: "Service".to_string(),
                name: "sandbox-gw".to_string(),
            }],
            load_balancer: None,
            health_check: hc.then(|| ServiceHealthCheck {
                active: Some(ActiveHealthCheckConfig::default()),
            }),
            outlier_detection: None,
            upstream_authority: ua,
            retry_constraint: None,
            circuit_breaker: None,
            connection: None,
        };

        // Valid, no health check.
        let ok = UpstreamAuthorityConfig {
            pattern: "*.sandbox.internal".to_string(),
            template: "${ctx:tenant}.sandbox.internal".to_string(),
            health_check_host: None,
        };
        assert!(base(Some(ok.clone()), false).get_validation_errors().is_empty());

        // Same config, but an active health check now demands healthCheckHost.
        let errs = base(Some(ok), true).get_validation_errors();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("healthCheckHost"), "got: {}", errs[0]);

        // A template whose suffix does not match the pattern is rejected.
        let bad = UpstreamAuthorityConfig {
            pattern: "*.sandbox.internal".to_string(),
            template: "${ctx:tenant}.evil.com".to_string(),
            health_check_host: None,
        };
        let errs = base(Some(bad), false).get_validation_errors();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("upstreamAuthority"), "got: {}", errs[0]);

        // Absent section => unchanged behaviour, no errors.
        assert!(base(None, true).get_validation_errors().is_empty());
    }

    #[test]
    fn retry_constraint_empty_object_uses_upstream_defaults() {
        let spec: EdgionBackendTrafficPolicySpec =
            serde_yaml::from_str("targetRefs:\n  - kind: Service\n    name: x\nretryConstraint: {}").unwrap();
        let rc = spec.retry_constraint.as_ref().expect("present");
        assert_eq!(rc.budget.percent, 20);
        assert_eq!(rc.budget.interval, "10s");
        let f = rc.min_retry_rate.as_ref().expect("floor default present");
        assert_eq!(f.count, 10);
        assert_eq!(f.interval, "1s");
        assert!(spec.get_validation_errors().is_empty());
    }

    #[test]
    fn retry_constraint_validation_rejects_bad_values() {
        let bad = |y: &str| -> Vec<String> {
            serde_yaml::from_str::<EdgionBackendTrafficPolicySpec>(y)
                .unwrap()
                .get_validation_errors()
        };
        // percent > 100
        assert!(!bad("targetRefs:\n  - kind: Service\n    name: x\nretryConstraint:\n  budget:\n    percent: 101\n    interval: 10s").is_empty());
        // budget.interval < 1s
        assert!(bad("targetRefs:\n  - kind: Service\n    name: x\nretryConstraint:\n  budget:\n    percent: 20\n    interval: 500ms").iter().any(|e| e.contains("budget.interval")));
        // budget.interval > 1h
        assert!(bad("targetRefs:\n  - kind: Service\n    name: x\nretryConstraint:\n  budget:\n    percent: 20\n    interval: 2h").iter().any(|e| e.contains("budget.interval")));
        // minRetryRate.count == 0
        assert!(bad("targetRefs:\n  - kind: Service\n    name: x\nretryConstraint:\n  budget:\n    interval: 10s\n  minRetryRate:\n    count: 0\n    interval: 1s").iter().any(|e| e.contains("minRetryRate.count")));
        // minRetryRate.count > 1_000_000
        assert!(bad("targetRefs:\n  - kind: Service\n    name: x\nretryConstraint:\n  budget:\n    interval: 10s\n  minRetryRate:\n    count: 1000001\n    interval: 1s").iter().any(|e| e.contains("minRetryRate.count")));
        // unparseable interval
        assert!(
            !bad("targetRefs:\n  - kind: Service\n    name: x\nretryConstraint:\n  budget:\n    interval: nope")
                .is_empty()
        );
    }

    #[test]
    fn connection_connect_timeout_deserializes_and_validates() {
        let spec: EdgionBackendTrafficPolicySpec =
            serde_yaml::from_str("targetRefs:\n  - kind: Service\n    name: x\nconnection:\n  connectTimeout: 250ms")
                .unwrap();
        assert_eq!(
            spec.connection.as_ref().unwrap().connect_timeout.as_deref(),
            Some("250ms")
        );
        assert!(spec.get_validation_errors().is_empty());

        // Empty connection object is valid (no override).
        let empty: EdgionBackendTrafficPolicySpec =
            serde_yaml::from_str("targetRefs:\n  - kind: Service\n    name: x\nconnection: {}").unwrap();
        assert!(empty.connection.as_ref().unwrap().connect_timeout.is_none());
        assert!(empty.get_validation_errors().is_empty());
    }

    #[test]
    fn connection_connect_timeout_validation_rejects_bad_values() {
        let bad = |y: &str| -> Vec<String> {
            serde_yaml::from_str::<EdgionBackendTrafficPolicySpec>(y)
                .unwrap()
                .get_validation_errors()
        };
        // 0s rejected
        assert!(
            bad("targetRefs:\n  - kind: Service\n    name: x\nconnection:\n  connectTimeout: 0s")
                .iter()
                .any(|e| e.contains("connection.connectTimeout"))
        );
        // > 1h rejected
        assert!(
            bad("targetRefs:\n  - kind: Service\n    name: x\nconnection:\n  connectTimeout: 2h")
                .iter()
                .any(|e| e.contains("connection.connectTimeout"))
        );
        // unparseable rejected
        assert!(!bad("targetRefs:\n  - kind: Service\n    name: x\nconnection:\n  connectTimeout: nope").is_empty());
    }
}
