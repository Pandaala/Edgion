//! EdgionBackendTrafficPolicy — per-destination traffic policy (LB algorithm,
//! active health check, outlier detection, panic threshold) attached to
//! Services via targetRefs. Design:
//! docs/superpowers/specs/2026-07-10-edgion-backend-traffic-policy-design.md

use super::common::PolicyAncestorStatus;
use super::health_check::ServiceHealthCheck;
use super::lb_policy::{ConsistentHashOn, ParsedLBPolicy};
use super::outlier_detection::OutlierDetectionConfig;
use super::upstream_authority::{validate_upstream_authority, UpstreamAuthorityConfig};
use kube::core::ObjectMeta;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
    /// Services this policy attaches to (same namespace as the policy).
    pub target_refs: Vec<PolicyTargetRef>,
    /// Load-balancing algorithm and panic threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_balancer: Option<LoadBalancerConfig>,
    /// Active health check (probe) configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_check: Option<ServiceHealthCheck>,
    /// Outlier detection (passive ejection) configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outlier_detection: Option<OutlierDetectionConfig>,
    /// Dynamic upstream authority (Host + SNI) within a wildcard pattern the
    /// backend declares. Absent = today's behaviour, unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_authority: Option<UpstreamAuthorityConfig>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PolicyTargetRef {
    /// Group of the referent. Empty string (default) = core API group.
    #[serde(default)]
    pub group: String,
    /// Kind of the referent. v1 supports only "Service".
    pub kind: String,
    /// Name of the referent Service (same namespace as the policy).
    pub name: String,
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
/// is the Gateway whose routes reach a target Service; the same policy can
/// be Accepted on one Gateway and Conflicted on another, which a flat
/// condition list cannot express.
#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EdgionBackendTrafficPolicyStatus {
    /// One entry per ancestor Gateway, stamped with this controller's name.
    /// Empty when no route references any target Service. Capped at 16.
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
        for tr in &self.target_refs {
            if !(tr.group.is_empty() || tr.group == "core") || tr.kind != "Service" {
                errors.push(format!(
                    "targetRef {}/{} {}: only core-group Service targets are supported",
                    tr.group, tr.kind, tr.name
                ));
            }
            if tr.name.is_empty() {
                errors.push("targetRef name must not be empty".to_string());
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
        if let Some(ua) = &self.upstream_authority {
            let has_active_hc = self.health_check.as_ref().map(|h| h.active.is_some()).unwrap_or(false);
            errors.extend(validate_upstream_authority(ua, has_active_hc));
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
}
