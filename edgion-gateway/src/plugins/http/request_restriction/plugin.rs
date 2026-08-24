//! Request Restriction plugin implementation
//!
//! This plugin restricts access based on request attributes and identity,
//! evaluated through the shared bounded condition-set grammar.
//!
//! ## Model (DNF: OR-of-AND)
//! - `mode` is required: `Deny` (match => reject, unmatched passes) or `Allow`
//!   (match => pass, unmatched is rejected). It has no default, so a config that
//!   omits it is rejected instead of silently picking a polarity.
//! - `conditions` has exactly one root `allOf` or `anyOf`.
//! - A root `anyOf` contains direct conditions or one-level named `allOf`
//!   groups. Configuration order determines short-circuit order.
//!
//! ## Configuration Examples:
//!
//! ### Deny: block bots OR /admin paths:
//! ```yaml
//! requestRestriction:
//!   mode: Deny
//!   conditions:
//!     anyOf:
//!       - type: keyMatch
//!         key: { type: path }
//!         values: ["/admin"]
//!       - name: destructive-admin
//!         allOf:
//!           - type: keyMatch
//!             key: { type: method }
//!             values: ["DELETE"]
//!           - type: keyMatch
//!             key: { type: path }
//!             regex: ["^/admin(?:/|$)"]
//!   message: "Bot access denied"
//! ```

use async_trait::async_trait;
use bytes::Bytes;

use crate::plugins::http::common::auth_common::send_gateway_plain_error_response;
use crate::plugins::runtime::conditions::{condition_set_has_unresolved, CompiledConditionSet, MatchState};
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{BodyRequirement, RequestRestrictionConfig};

/// Request Restriction plugin
pub struct RequestRestriction {
    name: String,
    config: RequestRestrictionConfig,
    conditions: CompiledConditionSet,
    /// Validity computed once at construction. `Some` means the config is
    /// malformed (e.g. an invalid regex) and the plugin must fail closed.
    /// The config is immutable for the plugin's lifetime (a reload builds a new
    /// instance), so this is safe to cache and avoids re-validating per request.
    config_error: Option<String>,
}

impl RequestRestriction {
    pub(crate) fn would_fail_construction(config: &RequestRestrictionConfig) -> bool {
        let mut validated = config.clone();
        validated.validate().is_err()
            || (!validated.allow_mode() && condition_set_has_unresolved(&validated.conditions))
    }

    /// Create a new RequestRestriction plugin from configuration
    pub fn create(config: &RequestRestrictionConfig) -> Box<dyn RequestFilter> {
        Self::create_with_body(config, None)
    }

    pub fn create_with_body(
        config: &RequestRestrictionConfig,
        body_requirement: Option<BodyRequirement>,
    ) -> Box<dyn RequestFilter> {
        // Clone and validate the config to compile regex patterns; capture (not
        // discard) the validity so run_request can fail closed without recompiling.
        let mut validated_config = config.clone();
        let mut config_error = validated_config.validate().err();
        let conditions = CompiledConditionSet::new(&validated_config.conditions, "conditions", body_requirement);
        if !validated_config.allow_mode() && conditions.has_unresolved_refs() {
            config_error.get_or_insert_with(|| "deny restriction contains unresolved references".to_string());
        }

        let plugin = RequestRestriction {
            name: "RequestRestriction".to_string(),
            config: validated_config,
            conditions,
            config_error,
        };

        Box::new(plugin)
    }
}

#[async_trait]
impl RequestFilter for RequestRestriction {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        if self.config_error.is_some() {
            plugin_log.push_err("restriction config invalid");
            let body = Bytes::from(r#"{"message":"Internal restriction policy error"}"#);
            if let Err(_e) = send_gateway_plain_error_response(session, 500, "application/json", body).await {
                plugin_log.push_err("write response fail");
            }
            return PluginRunningResult::ErrTerminateRequest;
        }

        // Apply polarity only after the shared tri-state evaluator completes.
        let allow_mode = self.config.allow_mode();
        let eval = match self
            .conditions
            .evaluate_async(session, plugin_log, self.conditions.atomic_count() > 1)
            .await
        {
            Ok(result) => result,
            Err(terminal) => return terminal,
        };
        let any_matched = eval.state == MatchState::True;
        let denied = any_matched != allow_mode;

        if denied {
            let deny_msg = if any_matched {
                format!("Deny {}", eval.cause.unwrap_or("conditions"))
            } else {
                "Deny (no allow alternative matched)".to_string()
            };
            plugin_log.push(&deny_msg);

            let message = self
                .config
                .message
                .as_deref()
                .unwrap_or("Access denied by restriction policy");

            // invariant: `check_fields` enforces 100 <= status < 600 before the plugin is
            // built, and a failed validation short-circuits above, so the helper's
            // `ResponseHeader::build` cannot fail on the configured status.
            let body = Bytes::from(serde_json::json!({"message": message}).to_string());

            if let Err(_e) =
                send_gateway_plain_error_response(session, self.config.status, "application/json", body).await
            {
                plugin_log.push_err("write response fail");
            }

            return PluginRunningResult::ErrTerminateRequest;
        }

        plugin_log.push("Allow");
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::common::KeyGet;
    use edgion_resources::resources::edgion_plugins::conditions::{
        AllOfGroup, AnyOfClause, Condition, ConditionSet, IpAccessMatch, KeyExistCondition, KeyMatchCondition,
        MatchCore,
    };
    use edgion_resources::resources::edgion_plugins::RestrictionMode;

    fn core_values(values: &[&str]) -> MatchCore {
        MatchCore {
            values: Some(values.iter().map(|s| s.to_string()).collect()),
            ..MatchCore::default()
        }
    }

    fn core_regex(patterns: &[&str]) -> MatchCore {
        MatchCore {
            regex: Some(patterns.iter().map(|s| s.to_string()).collect()),
            ..MatchCore::default()
        }
    }

    fn key_match(key: KeyGet, values: &[&str]) -> Condition {
        Condition::KeyMatch(KeyMatchCondition {
            key,
            core: core_values(values),
            negate: false,
            descriptor: Box::from(""),
        })
    }

    fn key_exist(key: KeyGet, negate: bool) -> Condition {
        Condition::KeyExist(KeyExistCondition {
            key,
            negate,
            descriptor: Box::from(""),
        })
    }

    fn named_group(name: &str, all_of: Vec<Condition>) -> AnyOfClause {
        AnyOfClause::AllOf(AllOfGroup {
            name: Some(name.to_string()),
            all_of,
        })
    }

    fn cfg(mode: RestrictionMode, groups: Vec<AnyOfClause>) -> RequestRestrictionConfig {
        let mut c = RequestRestrictionConfig {
            mode: Some(mode),
            conditions: ConditionSet::AnyOf { any_of: groups },
            ..Default::default()
        };
        let _ = c.validate();
        c
    }

    fn expect_key(session: &mut MockPluginSession, want: KeyGet, value: Option<String>) {
        session
            .expect_key_get()
            .withf(move |key| format!("{key:?}") == format!("{want:?}"))
            .return_const(value.clone());
    }

    #[tokio::test]
    async fn matching_sync_all_of_logs_compiled_descriptors() {
        let config = cfg(
            RestrictionMode::Deny,
            vec![named_group(
                "audited",
                vec![
                    key_match(KeyGet::Method, &["DELETE"]),
                    key_exist(
                        KeyGet::Header {
                            name: "X-Audit-Token".to_string(),
                        },
                        false,
                    ),
                ],
            )],
        );

        let plugin = RequestRestriction::create(&config);
        let mut session = MockPluginSession::new();
        session.expect_key_get().returning(|key| match key {
            KeyGet::Method => Some("DELETE".to_string()),
            KeyGet::Header { name } if name == "X-Audit-Token" => Some("present".to_string()),
            _ => None,
        });
        session.expect_write_response_header().returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});
        let mut log = PluginLog::new("RequestRestriction");

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrTerminateRequest
        );
        assert!(log.contains("conditions.anyOf[0].allOf[0]:keyMatch:method"));
        assert!(log.contains("conditions.anyOf[0].allOf[1]:keyExist:hdr:X-Audit-Token"));
        assert!(log.contains("audited"));
    }

    #[tokio::test]
    async fn unmatched_sync_predicate_does_not_log_descriptor() {
        let config = cfg(
            RestrictionMode::Deny,
            vec![named_group("readonly", vec![key_match(KeyGet::Method, &["GET"])])],
        );

        let plugin = RequestRestriction::create(&config);
        let mut session = MockPluginSession::new();
        expect_key(&mut session, KeyGet::Method, Some("POST".to_string()));
        let mut log = PluginLog::new("RequestRestriction");

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert!(!log.contains("0:keyMatch:method"));
        assert!(log.contains("Allow"));
    }

    #[tokio::test]
    async fn test_deny_mode_missing_header_match_denies() {
        // keyExist + negate: true => absent X-Auth-Token counts as a hit => deny.
        let config = cfg(
            RestrictionMode::Deny,
            vec![named_group(
                "missing-token",
                vec![key_exist(
                    KeyGet::Header {
                        name: "X-Auth-Token".to_string(),
                    },
                    true,
                )],
            )],
        );
        let plugin = RequestRestriction::create(&config);
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("RequestRestriction");
        expect_key(
            &mut s,
            KeyGet::Header {
                name: "X-Auth-Token".to_string(),
            },
            None,
        );
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});

        let result = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("Deny"));
    }

    #[tokio::test]
    async fn test_request_path_source_uses_entry_raw() {
        // keyMatch + requestPath must read the entry-time raw path.
        let config = cfg(
            RestrictionMode::Deny,
            vec![named_group(
                "deny-raw-path",
                vec![key_match(KeyGet::RequestPath, &["/%61dmin"])],
            )],
        );
        let plugin = RequestRestriction::create(&config);
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("RequestRestriction");
        expect_key(&mut s, KeyGet::RequestPath, Some("/%61dmin".to_string()));
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});

        let result = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("Deny"));
    }

    #[tokio::test]
    async fn test_deny_mode_and_group_needs_all_predicates() {
        // Group = (Method == DELETE) AND (Path == /admin). DELETE /other must pass.
        let config = cfg(
            RestrictionMode::Deny,
            vec![named_group(
                "delete-admin",
                vec![
                    key_match(KeyGet::Method, &["DELETE"]),
                    key_match(KeyGet::Path, &["/admin"]),
                ],
            )],
        );
        let plugin = RequestRestriction::create(&config);
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("RequestRestriction");
        s.expect_key_get().returning(|key| match key {
            KeyGet::Method => Some("DELETE".to_string()),
            KeyGet::Path => Some("/other".to_string()),
            _ => None,
        });

        let result = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(log.contains("Allow"));
    }

    #[tokio::test]
    async fn test_allow_mode_denies_when_no_group_matches() {
        // Allow mode: only GET passes. POST => deny.
        let config = cfg(
            RestrictionMode::Allow,
            vec![named_group("readonly", vec![key_match(KeyGet::Method, &["GET"])])],
        );
        let plugin = RequestRestriction::create(&config);
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("RequestRestriction");
        expect_key(&mut s, KeyGet::Method, Some("POST".to_string()));
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});

        let result = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("Deny"));
    }

    #[tokio::test]
    async fn test_allow_mode_multi_group_or_passes_on_second_group() {
        // Allow-mode OR across groups: (Method GET) OR (X-Tier gold). A POST with
        // X-Tier: gold matches the second group => pass.
        let config = cfg(
            RestrictionMode::Allow,
            vec![
                named_group("readonly", vec![key_match(KeyGet::Method, &["GET"])]),
                named_group(
                    "gold-tier",
                    vec![key_match(
                        KeyGet::Header {
                            name: "X-Tier".to_string(),
                        },
                        &["gold"],
                    )],
                ),
            ],
        );
        let plugin = RequestRestriction::create(&config);
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("RequestRestriction");
        s.expect_key_get().returning(|key| match key {
            KeyGet::Method => Some("POST".to_string()),
            KeyGet::Header { name } if name == "X-Tier" => Some("gold".to_string()),
            _ => None,
        });

        let result = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(log.contains("Allow"));
    }

    #[tokio::test]
    async fn deny_mode_unresolved_reference_fails_closed() {
        let unresolved: Condition = serde_json::from_value(serde_json::json!({
            "type": "remoteIpMatch",
            "ipRefs": [{ "name": "missing" }],
            "negate": true
        }))
        .unwrap();
        let config = cfg(RestrictionMode::Deny, vec![named_group("blocked", vec![unresolved])]);
        let plugin = RequestRestriction::create(&config);
        let mut session = MockPluginSession::new();
        session.expect_write_response_header().returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});
        let mut log = PluginLog::new("RequestRestriction");

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrTerminateRequest
        );
        assert!(log.contains("restriction config invalid"));
    }

    #[tokio::test]
    async fn allow_mode_skips_unresolved_group_and_uses_backup_group() {
        let unresolved: Condition = serde_json::from_value(serde_json::json!({
            "type": "secretMatch",
            "key": { "type": "header", "name": "X-Token" },
            "secretKey": "token",
            "secretRef": { "name": "missing" },
            "negate": true
        }))
        .unwrap();
        let config = cfg(
            RestrictionMode::Allow,
            vec![
                named_group("incomplete", vec![unresolved]),
                named_group("backup", vec![key_match(KeyGet::Method, &["GET"])]),
            ],
        );
        let plugin = RequestRestriction::create(&config);
        let mut session = MockPluginSession::new();
        expect_key(&mut session, KeyGet::Method, Some("GET".to_string()));
        let mut log = PluginLog::new("RequestRestriction");

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }

    #[tokio::test]
    async fn allow_mode_skips_unresolved_body_hmac_group() {
        let unresolved: Condition = serde_json::from_value(serde_json::json!({
            "type": "hmacAuth",
            "secretGroups": [{
                "name": "default",
                "secretRefs": [{ "name": "missing" }]
            }],
            "validateRequestBody": true,
            "negate": true
        }))
        .unwrap();
        let config = cfg(
            RestrictionMode::Allow,
            vec![
                named_group("incomplete", vec![unresolved]),
                named_group("backup", vec![key_match(KeyGet::Method, &["GET"])]),
            ],
        );
        let plugin = RequestRestriction::create_with_body(&config, Some(BodyRequirement::fail_close()));
        let mut session = MockPluginSession::new();
        expect_key(&mut session, KeyGet::Method, Some("GET".to_string()));
        let mut log = PluginLog::new("RequestRestriction");

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }

    #[tokio::test]
    async fn test_invalid_config_fails_closed() {
        // An invalid regex fails validation at construction; run_request must fail
        // closed (terminate) rather than evaluate a half-compiled config.
        let config = cfg(
            RestrictionMode::Deny,
            vec![named_group(
                "bad",
                vec![Condition::KeyMatch(KeyMatchCondition {
                    key: KeyGet::Header { name: "X".to_string() },
                    core: core_regex(&["[invalid"]),
                    negate: false,
                    descriptor: Box::from(""),
                })],
            )],
        );
        let plugin = RequestRestriction::create(&config);
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("RequestRestriction");
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});

        let result = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    }

    #[tokio::test]
    async fn rule_groups_evaluated_in_config_order_first_match_short_circuits() {
        // Both groups would match GET. Configuration order and first-match
        // short-circuit pin the deny log to alpha; beta is never considered.
        let config = cfg(
            RestrictionMode::Deny,
            vec![
                named_group("alpha", vec![key_match(KeyGet::Method, &["GET"])]),
                named_group("beta", vec![key_match(KeyGet::Method, &["GET"])]),
            ],
        );
        let plugin = RequestRestriction::create(&config);
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("RequestRestriction");
        expect_key(&mut s, KeyGet::Method, Some("GET".to_string()));
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});

        let result = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("Deny alpha"));
        assert!(!log.contains("beta"));
    }

    #[tokio::test]
    async fn ip_and_path_in_one_group() {
        let config = cfg(
            RestrictionMode::Deny,
            vec![named_group(
                "ip-and-path",
                vec![
                    Condition::DirectPeerIpMatch(IpAccessMatch {
                        ips: vec!["203.0.113.10".to_string()],
                        ..Default::default()
                    }),
                    key_match(KeyGet::Path, &["/admin"]),
                ],
            )],
        );
        let plugin = RequestRestriction::create(&config);

        let mut hit = MockPluginSession::new();
        let mut hit_log = PluginLog::new("RequestRestriction");
        hit.expect_direct_peer_ip().return_const("203.0.113.10".to_string());
        hit.expect_key_get().returning(|key| match key {
            KeyGet::Path => Some("/admin".to_string()),
            _ => None,
        });
        hit.expect_write_response_header().returning(|_, _| Ok(()));
        hit.expect_write_response_body().returning(|_, _| Ok(()));
        hit.expect_shutdown().returning(|| {});
        assert_eq!(
            plugin.run_request(&mut hit, &mut hit_log).await,
            PluginRunningResult::ErrTerminateRequest
        );
        assert!(hit_log.contains("Deny"));

        let mut path_miss = MockPluginSession::new();
        let mut path_log = PluginLog::new("RequestRestriction");
        path_miss
            .expect_direct_peer_ip()
            .return_const("203.0.113.10".to_string());
        path_miss.expect_key_get().returning(|key| match key {
            KeyGet::Path => Some("/public".to_string()),
            _ => None,
        });
        assert_eq!(
            plugin.run_request(&mut path_miss, &mut path_log).await,
            PluginRunningResult::GoodNext
        );
        assert!(path_log.contains("Allow"));
    }

    /// Signed HMAC-body must match. The shared header-only evaluator returns
    /// false whenever `validateRequestBody` is set; this test fails on that path.
    #[tokio::test]
    async fn hmac_body_all_of_matches_signed_request() {
        use base64::engine::general_purpose::STANDARD;
        use base64::Engine;
        use edgion_resources::resources::edgion_plugins::hmac_match::HmacMatchConfig;
        use edgion_resources::resources::edgion_plugins::{HmacAlgorithm, HmacCredential};
        use hmac::{Hmac, Mac};
        use sha2::{Digest, Sha256};
        use std::collections::HashMap;

        const SECRET: &[u8] = b"alice-super-secret-key";
        let body = b"{\"n\":1}";
        let date = chrono::Utc::now().to_rfc2822();
        let digest = format!("SHA-256={}", STANDARD.encode(Sha256::digest(body)));
        let signing = format!("date: {date}\ndigest: {digest}");
        let mut mac = Hmac::<Sha256>::new_from_slice(SECRET).unwrap();
        mac.update(signing.as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());
        let auth = format!(
            r#"hmac username="alice", algorithm="hmac-sha256", headers="date digest", signature="{signature}""#
        );

        let hmac = HmacMatchConfig {
            algorithms: vec![HmacAlgorithm::HmacSha256],
            clock_skew: 600,
            validate_request_body: true,
            resolved_credentials: Some(HashMap::from([(
                "alice".to_string(),
                HmacCredential {
                    secret: SECRET.to_vec(),
                    headers: HashMap::new(),
                    group: "default".to_string(),
                },
            )])),
            ..Default::default()
        };
        let config = cfg(
            RestrictionMode::Deny,
            vec![named_group("signed-body", vec![Condition::HmacAuth(hmac)])],
        );
        let plugin = RequestRestriction::create_with_body(&config, Some(BodyRequirement::fail_close()));

        let mut session = MockPluginSession::new();
        let auth_c = auth.clone();
        let date_c = date.clone();
        let digest_c = digest.clone();
        session.expect_header_value().returning(move |name| match name {
            "authorization" => Some(auth_c.clone()),
            "proxy-authorization" => None,
            "date" => Some(date_c.clone()),
            "digest" => Some(digest_c.clone()),
            _ => None,
        });
        session.expect_get_path().return_const("/v1/items".to_string());
        session.expect_get_query().returning(|| None);
        session.expect_get_method().return_const("POST".to_string());
        session
            .expect_request_body_sha256()
            .returning(move || Ok(Sha256::digest(body).into()));
        session
            .expect_ensure_request_body()
            .returning(|_| Ok(body.len() as u64));
        session.expect_write_response_header().returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});

        let mut log = PluginLog::new("RequestRestriction");
        let result = plugin.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::ErrTerminateRequest, "{log:?}");
        assert!(
            !log.contains("hmacAuth"),
            "single-leaf match descriptor must be suppressed: {log:?}"
        );
        assert!(log.contains("Deny"), "{log:?}");
    }

    #[tokio::test]
    async fn negated_hmac_body_no_match_logs_descriptor() {
        use edgion_resources::resources::edgion_plugins::hmac_match::HmacMatchConfig;
        use edgion_resources::resources::edgion_plugins::HmacCredential;
        use std::collections::HashMap;

        let hmac = HmacMatchConfig {
            validate_request_body: true,
            negate: true,
            resolved_credentials: Some(HashMap::from([(
                "alice".to_string(),
                HmacCredential {
                    secret: b"alice-super-secret-key".to_vec(),
                    headers: HashMap::new(),
                    group: "default".to_string(),
                },
            )])),
            ..Default::default()
        };
        let config = cfg(
            RestrictionMode::Deny,
            vec![named_group("unsigned-body", vec![Condition::HmacAuth(hmac)])],
        );
        let plugin = RequestRestriction::create_with_body(&config, Some(BodyRequirement::fail_close()));

        let mut session = MockPluginSession::new();
        session.expect_header_value().returning(|_| None);
        session.expect_write_response_header().returning(|_, _| Ok(()));
        session.expect_write_response_body().returning(|_, _| Ok(()));
        session.expect_shutdown().returning(|| {});
        let mut log = PluginLog::new("RequestRestriction");

        assert_eq!(
            plugin.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrTerminateRequest,
            "{log:?}"
        );
        assert!(
            !log.contains("hmacAuth"),
            "single-leaf match descriptor must be suppressed: {log:?}"
        );
        assert!(log.contains("Deny"), "{log:?}");
    }
}
