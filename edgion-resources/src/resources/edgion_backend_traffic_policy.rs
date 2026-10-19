//! EdgionBackendTrafficPolicy — per-destination traffic policy (LB algorithm,
//! active health check, outlier detection, degrade threshold) attached to
//! Services or EdgionBackend resources via targetRefs. Design:
//! docs/superpowers/specs/2026-07-10-edgion-backend-traffic-policy-design.md

use super::circuit_breaker::CircuitBreakerConfig;
use super::common::PolicyAncestorStatus;
use super::health_check::ServiceHealthCheck;
use super::lb_policy::{ConsistentHashOn, ParsedLBPolicy};
use super::outlier_detection::OutlierDetectionConfig;
use super::upstream_authority::{validate_upstream_authority, UpstreamAuthorityConfig};
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
    /// Controller-computed status for this deployment, transported to the Gateway.
    /// Never supplied by operators or used as the persisted status history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub current_status: Option<EdgionBackendTrafficPolicyStatus>,

    /// Complete Controller-derived creation instant for policy arbitration.
    /// Recomputed from source metadata on every parse; not operator input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub prepared_precedence: Option<super::common::policy_precedence::PreparedPolicyPrecedence>,

    /// Controller-owned attachment capacity decision; never operator authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub prepared_ancestor_admission: Option<super::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission>,

    /// Homogeneous Service or EdgionBackend targets in the policy namespace.
    #[schemars(length(min = 1))]
    pub target_refs: Vec<PolicyTargetRef>,
    /// Controller-resolved homogeneous target capability. Input values are
    /// cleared and recomputed before publication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_target_class: Option<ResolvedPolicyTargetClass>,
    /// Load balancing for core Service targets.
    /// Panic threshold is supported only for direct core Service targets.
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
    /// Service backend declares. Supported for core Service targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_authority: Option<UpstreamAuthorityConfig>,
    /// Retry budget: cap retry volume as a percentage of request volume to the
    /// target backend over a sliding window. Absent = today's behaviour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_constraint: Option<RetryConstraintConfig>,
    /// Circuit breaker: hard cap on concurrent in-flight requests per Service
    /// endpoint or per EdgionBackend. Absent = no concurrency cap.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Kind of the referent: "Service" or "EdgionBackend".
    pub kind: String,
    /// Name of the referent (same namespace as the policy).
    pub name: String,
}

/// Canonical kind of an EdgionBackendTrafficPolicy target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PolicyTargetKind {
    Service,
    EdgionBackend,
}

/// Controller-resolved capability for one atomically publishable policy.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ResolvedPolicyTargetClass {
    Service,
    Ai,
}

impl PolicyTargetKind {
    pub const fn group(self) -> &'static str {
        match self {
            Self::Service => "",
            Self::EdgionBackend => "edgion.io",
        }
    }

    pub const fn kind(self) -> &'static str {
        match self {
            Self::Service => "Service",
            Self::EdgionBackend => "EdgionBackend",
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
    /// EdgionGatewayConfig.spec.loadBalancing.degradeThreshold for the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degrade_threshold: Option<u8>,
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
    pub fn get_validation_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.target_refs.is_empty() {
            errors.push("targetRefs must not be empty".to_string());
        }

        let mut target_kinds = HashSet::new();
        let mut targets = HashSet::new();
        let mut duplicate_targets = Vec::new();
        for tr in &self.target_refs {
            match tr.target_kind() {
                Ok(target_kind) => {
                    target_kinds.insert(target_kind);
                    if !targets.insert((target_kind, tr.name.as_str())) {
                        duplicate_targets.push((target_kind, tr.name.as_str()));
                    }
                }
                Err(error) => errors.push(error),
            }
            if tr.name.is_empty() {
                errors.push("targetRef name must not be empty".to_string());
            } else if !super::common::kubernetes_name::is_dns_subdomain(&tr.name, 253) {
                errors.push("targetRef name must be a valid resource name".to_string());
            }
        }
        if target_kinds.len() > 1 {
            errors.push("targetRefs must all have the same target kind".to_string());
        }

        let targets_ai = target_kinds.len() == 1 && target_kinds.contains(&PolicyTargetKind::EdgionBackend);
        if targets_ai {
            if self.target_refs.len() > 16 {
                errors.push("EdgionBackend targetRefs must contain at most 16 entries".to_string());
            }
            for (target_kind, target_name) in duplicate_targets {
                errors.push(format!(
                    "EdgionBackend targetRefs must not contain duplicate target {}/{} {}",
                    target_kind.group(),
                    target_kind.kind(),
                    target_name
                ));
            }
            if self.health_check.is_some() {
                errors.push("healthCheck is not supported for EdgionBackend targets".to_string());
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
            if let Some(pt) = lb.degrade_threshold {
                if pt > 100 {
                    errors.push("loadBalancer.degradeThreshold must be 0-100".to_string());
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
                    HashOn::Header => ConsistentHashOn::Header(ch.key.as_deref().unwrap_or_default().into()),
                    HashOn::Cookie => ConsistentHashOn::Cookie(ch.key.as_deref().unwrap_or_default().into()),
                    HashOn::QueryParam => ConsistentHashOn::Arg(ch.key.as_deref().unwrap_or_default().into()),
                    HashOn::SourceIp => ConsistentHashOn::SourceIp,
                };
                Some(ParsedLBPolicy::ConsistentHash(on))
            }
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/resources/edgion_backend_traffic_policy_test.rs"]
mod tests;
