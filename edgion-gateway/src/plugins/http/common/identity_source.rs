//! Shared per-source value extraction for the restriction match engine.
//!
//! Both `request_restriction` and `global_access_control` build their candidate
//! match values through `extract_rule_values`, so the request-attribute and
//! identity source semantics cannot drift between the two plugins.

use crate::plugins::http::common::resolve_claim_path;
use crate::plugins::runtime::PluginSession;
use edgion_resources::resources::edgion_plugins::{RestrictionRule, RestrictionSource, SanType};

/// Extract the candidate match values for a rule from the live session.
///
/// An empty `Vec` means the source value is absent — the caller's
/// `RestrictionRule::evaluate_multi` then applies `on_missing`. Scalar sources
/// yield 0 or 1 element.
pub fn extract_rule_values(session: &dyn PluginSession, rule: &RestrictionRule) -> Vec<String> {
    match rule.source {
        RestrictionSource::Header => single(rule.key.as_deref().and_then(|k| session.header_value(k))),
        RestrictionSource::Cookie => single(rule.key.as_deref().and_then(|k| session.get_cookie(k))),
        RestrictionSource::Query => single(rule.key.as_deref().and_then(|k| session.get_query_param(k))),
        RestrictionSource::Path => single(Some(session.get_path().to_string())),
        RestrictionSource::RequestPath => single(Some(session.get_request_path().to_string())),
        RestrictionSource::Method => single(Some(session.get_method().to_string())),
        RestrictionSource::Referer => single(session.header_value("Referer")),
        RestrictionSource::MtlsSubject => single(session.client_cert_info().map(|c| c.subject)),
        RestrictionSource::MtlsCn => single(session.client_cert_info().and_then(|c| c.cn)),
        RestrictionSource::MtlsSan => extract_san_values(session, rule.san_type.as_ref()),
        RestrictionSource::JwtClaim => extract_jwt_claim_values(session, rule.key.as_deref()),
    }
}

/// JWT claim candidates from ctx. Reads `jwt_claims` (jwt_auth) first, then
/// `oidc_claims` (openid_connect). Requires the authenticating plugin to run
/// earlier in the chain with `store_claims_in_ctx: true`; otherwise missing.
fn extract_jwt_claim_values(session: &dyn PluginSession, key: Option<&str>) -> Vec<String> {
    let Some(path) = key else {
        return Vec::new();
    };
    let Some(raw) = session
        .get_ctx_var("jwt_claims")
        .or_else(|| session.get_ctx_var("oidc_claims"))
    else {
        return Vec::new();
    };
    let Ok(claims) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    match resolve_claim_path(&claims, path) {
        Some(node) => json_node_to_candidates(node),
        None => Vec::new(),
    }
}

/// Flatten a JSON node into match candidates. Scalars yield one value; arrays
/// yield each scalar element ("any element matches"); objects / null / nested
/// non-scalars yield nothing (missing).
fn json_node_to_candidates(node: &serde_json::Value) -> Vec<String> {
    match node {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Number(n) => vec![n.to_string()],
        serde_json::Value::Bool(b) => vec![b.to_string()],
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| match v {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Number(n) => Some(n.to_string()),
                serde_json::Value::Bool(b) => Some(b.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Wrap an optional scalar into a 0/1-element candidate list.
fn single(v: Option<String>) -> Vec<String> {
    match v {
        Some(s) => vec![s],
        None => Vec::new(),
    }
}

/// SAN candidate values, optionally filtered/normalized by type.
///
/// SAN storage format (see `tls/runtime/backend/cert_extractor.rs`): DNS names
/// are bare; `IP:` / `email:` / `uri:` carry a type prefix. Untyped matching
/// returns the raw stored strings; typed matching filters by prefix and strips
/// it. `DNS` = entries with no known prefix.
fn extract_san_values(session: &dyn PluginSession, san_type: Option<&SanType>) -> Vec<String> {
    let Some(cert) = session.client_cert_info() else {
        return Vec::new();
    };
    match san_type {
        None => cert.sans,
        Some(SanType::Dns) => cert.sans.into_iter().filter(|s| !has_known_san_prefix(s)).collect(),
        Some(SanType::Uri) => strip_san_prefix(cert.sans, "uri:"),
        Some(SanType::Email) => strip_san_prefix(cert.sans, "email:"),
    }
}

fn has_known_san_prefix(s: &str) -> bool {
    s.starts_with("uri:") || s.starts_with("email:") || s.starts_with("IP:")
}

fn strip_san_prefix(sans: Vec<String>, prefix: &str) -> Vec<String> {
    sans.into_iter()
        .filter_map(|s| s.strip_prefix(prefix).map(|v| v.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::ClientCertInfo;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::edgion_plugins::SanType;

    fn cert() -> ClientCertInfo {
        ClientCertInfo {
            subject: "CN=alice, O=acme".to_string(),
            sans: vec![
                "alice.example.com".to_string(),
                "uri:spiffe://edgion.io/ns/foo".to_string(),
                "email:alice@example.com".to_string(),
            ],
            cn: Some("alice".to_string()),
            fingerprint: "ab:cd".to_string(),
        }
    }

    fn rule(source: RestrictionSource) -> RestrictionRule {
        RestrictionRule {
            source,
            ..Default::default()
        }
    }

    fn jwt_rule(key: &str) -> RestrictionRule {
        RestrictionRule {
            source: RestrictionSource::JwtClaim,
            key: Some(key.to_string()),
            ..Default::default()
        }
    }

    fn values(session: &dyn PluginSession, rule: &RestrictionRule) -> Vec<String> {
        extract_rule_values(session, rule)
    }

    #[test]
    fn jwt_claim_nested_array_any_match() {
        let mut s = MockPluginSession::new();
        s.expect_get_ctx_var().returning(|k| {
            if k == "jwt_claims" {
                Some(r#"{"sub":"u1","realm_access":{"roles":["admin","dev"]}}"#.to_string())
            } else {
                None
            }
        });
        let out = values(&s, &jwt_rule("realm_access.roles"));
        assert_eq!(out, vec!["admin".to_string(), "dev".to_string()]);
    }

    #[test]
    fn jwt_claim_scalar_and_number_and_bool() {
        let mut s = MockPluginSession::new();
        s.expect_get_ctx_var().returning(|k| {
            if k == "jwt_claims" {
                Some(r#"{"sub":"u1","age":42,"active":true}"#.to_string())
            } else {
                None
            }
        });
        assert_eq!(values(&s, &jwt_rule("sub")), vec!["u1".to_string()]);
        assert_eq!(values(&s, &jwt_rule("age")), vec!["42".to_string()]);
        assert_eq!(values(&s, &jwt_rule("active")), vec!["true".to_string()]);
    }

    #[test]
    fn jwt_claim_falls_back_to_oidc_claims() {
        let mut s = MockPluginSession::new();
        s.expect_get_ctx_var().returning(|k| match k {
            "jwt_claims" => None,
            "oidc_claims" => Some(r#"{"groups":["g1"]}"#.to_string()),
            _ => None,
        });
        assert_eq!(values(&s, &jwt_rule("groups")), vec!["g1".to_string()]);
    }

    #[test]
    fn jwt_claim_missing_branches() {
        // No ctx var at all.
        let mut s = MockPluginSession::new();
        s.expect_get_ctx_var().returning(|_| None);
        assert!(values(&s, &jwt_rule("sub")).is_empty());

        // Path resolves to an object/null => missing.
        let mut s2 = MockPluginSession::new();
        s2.expect_get_ctx_var().returning(|k| {
            if k == "jwt_claims" {
                Some(r#"{"realm_access":{"roles":["x"]}}"#.to_string())
            } else {
                None
            }
        });
        assert!(values(&s2, &jwt_rule("realm_access")).is_empty());
        assert!(values(&s2, &jwt_rule("missing.path")).is_empty());
    }

    #[test]
    fn mtls_subject_and_cn() {
        let mut s = MockPluginSession::new();
        s.expect_client_cert_info().returning(|| Some(cert()));
        assert_eq!(
            values(&s, &rule(RestrictionSource::MtlsSubject)),
            vec!["CN=alice, O=acme".to_string()]
        );
    }

    #[test]
    fn mtls_missing_when_no_cert() {
        let mut s = MockPluginSession::new();
        s.expect_client_cert_info().returning(|| None);
        assert!(values(&s, &rule(RestrictionSource::MtlsCn)).is_empty());
    }

    #[test]
    fn mtls_san_untyped_returns_raw() {
        let mut s = MockPluginSession::new();
        s.expect_client_cert_info().returning(|| Some(cert()));
        let out = values(&s, &rule(RestrictionSource::MtlsSan));
        assert!(out.contains(&"uri:spiffe://edgion.io/ns/foo".to_string()));
        assert!(out.contains(&"alice.example.com".to_string()));
    }

    #[test]
    fn mtls_san_typed_strips_prefix() {
        let mut s = MockPluginSession::new();
        s.expect_client_cert_info().returning(|| Some(cert()));
        let mut r = rule(RestrictionSource::MtlsSan);
        r.san_type = Some(SanType::Uri);
        assert_eq!(values(&s, &r), vec!["spiffe://edgion.io/ns/foo".to_string()]);

        let mut dns = rule(RestrictionSource::MtlsSan);
        dns.san_type = Some(SanType::Dns);
        assert_eq!(values(&s, &dns), vec!["alice.example.com".to_string()]);
    }
}
