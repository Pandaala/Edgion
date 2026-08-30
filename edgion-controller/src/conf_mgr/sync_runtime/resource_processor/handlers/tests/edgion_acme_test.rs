//! Unit tests for `edgion_acme.rs` (see `super`).
use super::*;
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::resources::common::ParentReference;
use edgion_resources::resources::common::SecretObjectReference;
use edgion_resources::resources::edgion_acme::AcmeStorageConfig;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use kube::api::ObjectMeta;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

// ========================================================================
// Test helpers
// ========================================================================

fn make_ctx(secrets: HashMap<String, Secret>) -> (HandlerContext, Arc<ControllerRuntime>) {
    let runtime = Arc::new(ControllerRuntime::default());
    runtime.secret_store.replace_all(secrets);
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    (ctx, runtime)
}

fn make_secret(namespace: &str, name: &str, data: &[(&str, &[u8])]) -> (String, Secret) {
    let mut bmap = BTreeMap::new();
    for (k, v) in data {
        bmap.insert(k.to_string(), ByteString(v.to_vec()));
    }
    let secret = Secret {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            ..Default::default()
        },
        data: Some(bmap),
        ..Default::default()
    };
    let key = format!("{}/{}", namespace, name);
    (key, secret)
}

fn make_acme(ns: &str, name: &str, secret_name: &str) -> EdgionAcme {
    EdgionAcme {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(ns.to_string()),
            generation: Some(1),
            ..Default::default()
        },
        spec: edgion_resources::resources::edgion_acme::EdgionAcmeSpec {
            server: "https://acme-v02.api.letsencrypt.org/directory".to_string(),
            email: "admin@example.com".to_string(),
            private_key_secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: secret_name.to_string(),
                namespace: None,
            },
            domains: vec!["example.com".to_string()],
            key_type: Default::default(),
            challenge: AcmeChallenge::Http01 {
                gateway_ref: ParentReference {
                    group: None,
                    kind: None,
                    namespace: Some("default".to_string()),
                    name: "gw".to_string(),
                    section_name: None,
                    port: None,
                },
            },
            renewal: Default::default(),
            external_account_binding: None,
            storage: AcmeStorageConfig {
                secret_name: "cert".to_string(),
                secret_namespace: None,
            },
            auto_edgion_tls: Default::default(),
        },
        status: None,
    }
}

// ========================================================================
// Shared scenario type and driver for parse() table-driven tests
// ========================================================================

struct AcmeParseScenario {
    name: &'static str,
    secrets: Vec<(String, Secret)>,
    acme: EdgionAcme,
    expected_status: &'static str,
    expected_reason: Option<&'static str>,
    expected_message_substrings: &'static [&'static str],
    /// Each entry is a secret store key (e.g. "default/missing-secret").
    /// The driver asserts that the EdgionAcme owner is registered for that key.
    expected_refs_owner_for_keys: &'static [&'static str],
}

async fn run_acme_parse_scenario(scenario: AcmeParseScenario) {
    // `parse` reaches `notify_resource_changed` -> `get_sender()`, which reads the
    // process-global `ACME_SERVICE`. Serialize against the ACME service tests that
    // install a handle into that global (see `ACME_TEST_MUTEX`).
    let _acme_guard = crate::services::acme::service::ACME_TEST_MUTEX.lock().await;

    let secrets: HashMap<String, Secret> = scenario.secrets.into_iter().collect();
    let (ctx, _runtime) = make_ctx(secrets);
    let handler = EdgionAcmeHandler::new();

    let acme_ns = scenario.acme.metadata.namespace.clone().unwrap_or_default();
    let acme_name = scenario.acme.metadata.name.clone().unwrap_or_default();

    let result = handler.parse(scenario.acme, &ctx).await;

    let resolved = match result {
        ProcessResult::Continue(acme) => {
            let status = acme
                .status
                .unwrap_or_else(|| panic!("[{}] status should be set", scenario.name));
            status
                .conditions
                .into_iter()
                .find(|c| c.type_ == "ResolvedRefs")
                .unwrap_or_else(|| panic!("[{}] ResolvedRefs condition should exist", scenario.name))
        }
        ProcessResult::Skip { reason } => {
            panic!("[{}] expected Continue, got Skip: {reason}", scenario.name)
        }
    };

    assert_eq!(
        resolved.status, scenario.expected_status,
        "[{}] status mismatch",
        scenario.name
    );

    if let Some(expected_reason) = scenario.expected_reason {
        assert_eq!(resolved.reason, expected_reason, "[{}] reason mismatch", scenario.name);
    }

    for &substr in scenario.expected_message_substrings {
        assert!(
            resolved.message.contains(substr),
            "[{}] message missing {:?}, got: {}",
            scenario.name,
            substr,
            resolved.message
        );
    }

    let owner = ResourceRef::new(ResourceKind::EdgionAcme, Some(acme_ns), acme_name);
    for &secret_key in scenario.expected_refs_owner_for_keys {
        let refs = ctx.secret_ref_manager().get_refs(secret_key);
        assert!(
            refs.contains(&owner),
            "[{}] SecretRefManager should contain owner for key {secret_key}",
            scenario.name
        );
    }
}

// ========================================================================
// Test: account-branch parse scenarios (Http01 challenge)
// ========================================================================

#[tokio::test]
async fn test_acme_account_resolve_scenarios() {
    let scenarios: Vec<AcmeParseScenario> = vec![
        // 1. Happy path — Secret present with valid accountCredentials.
        AcmeParseScenario {
            name: "account_credential_happy_path",
            secrets: vec![make_secret(
                "default",
                "acme-account",
                &[("accountCredentials", br#"{"ok":true}"#)],
            )],
            acme: make_acme("default", "my-acme", "acme-account"),
            expected_status: "True",
            expected_reason: None,
            expected_message_substrings: &[],
            expected_refs_owner_for_keys: &[],
        },
        // 2. Secret not found → NeedsRegistration (Unknown).
        //    SecretRefManager must register the missing secret so the
        //    resource re-reconciles when it arrives.
        AcmeParseScenario {
            name: "secret_not_found_needs_registration",
            secrets: vec![],
            acme: make_acme("default", "my-acme", "missing-secret"),
            expected_status: "Unknown",
            expected_reason: Some("NeedsRegistration"),
            expected_message_substrings: &[],
            expected_refs_owner_for_keys: &["default/missing-secret"],
        },
        // 3. Secret exists but missing accountCredentials key → NeedsRegistration.
        AcmeParseScenario {
            name: "secret_missing_key_needs_registration",
            secrets: vec![make_secret("default", "acme-account", &[("wrongKey", b"some-value")])],
            acme: make_acme("default", "my-acme", "acme-account"),
            expected_status: "Unknown",
            expected_reason: Some("NeedsRegistration"),
            expected_message_substrings: &[],
            expected_refs_owner_for_keys: &[],
        },
        // 4. Corrupt account data → Continue + ResolvedRefs=False/DecodeError.
        //    ACME uses the generic DecodeError reason shared with
        //    LinkSys / EdgionPlugins / EdgionTls.
        AcmeParseScenario {
            name: "corrupt_account_data_returns_continue_with_resolved_refs_false",
            secrets: vec![make_secret(
                "default",
                "acme-account",
                &[("accountCredentials", b"not-valid-json")],
            )],
            acme: make_acme("default", "my-acme", "acme-account"),
            expected_status: "False",
            expected_reason: Some("DecodeError"),
            expected_message_substrings: &["decode failed"],
            expected_refs_owner_for_keys: &[],
        },
    ];

    for scenario in scenarios {
        run_acme_parse_scenario(scenario).await;
    }
}

// ========================================================================
// Test: DNS-branch parse scenarios (Dns01 challenge)
// ========================================================================
//
// These cover the four ResolveOutcome paths the DNS resolve branch can
// take — SecretNotFound, ReferenceGrantMissing, MissingKeys, and the
// happy path. The account Secret is present and valid where noted so
// the tests isolate the DNS branch's contribution to ResolvedRefs.

#[tokio::test]
async fn test_acme_dns_resolve_scenarios() {
    let (valid_account_key, valid_account_secret) =
        make_secret("default", "acme-account", &[("accountCredentials", br#"{"ok":true}"#)]);

    let scenarios: Vec<AcmeParseScenario> = vec![
        // 6. DNS Secret missing → ResolvedRefs=False/SecretNotFound.
        //    Account Secret present so account branch writes True first;
        //    DNS branch must override with False/SecretNotFound.
        AcmeParseScenario {
            name: "dns_credential_secret_not_found",
            secrets: vec![(valid_account_key.clone(), valid_account_secret.clone())],
            acme: make_acme_dns01("default", "my-acme", "acme-account", "cloudflare", "missing-dns", None),
            expected_status: "False",
            expected_reason: Some("SecretNotFound"),
            expected_message_substrings: &["DNS credential"],
            expected_refs_owner_for_keys: &["default/missing-dns"],
        },
        // 7. Cloudflare DNS Secret missing api-token → ResolvedRefs=False/MissingKeys.
        AcmeParseScenario {
            name: "dns_credential_missing_api_token",
            secrets: vec![
                (valid_account_key.clone(), valid_account_secret.clone()),
                make_secret("default", "cf-secret", &[("wrong-key", b"value")]),
            ],
            acme: make_acme_dns01("default", "my-acme", "acme-account", "cloudflare", "cf-secret", None),
            expected_status: "False",
            expected_reason: Some("MissingKeys"),
            expected_message_substrings: &["api-token"],
            expected_refs_owner_for_keys: &[],
        },
        // 8. AliDNS Secret missing both keys → message names both.
        AcmeParseScenario {
            name: "dns_credential_alidns_missing_both_keys",
            secrets: vec![
                (valid_account_key.clone(), valid_account_secret.clone()),
                make_secret("default", "alidns-secret", &[("unrelated", b"v")]),
            ],
            acme: make_acme_dns01("default", "my-acme", "acme-account", "alidns", "alidns-secret", None),
            expected_status: "False",
            expected_reason: Some("MissingKeys"),
            expected_message_substrings: &["access-key-id", "access-key-secret"],
            expected_refs_owner_for_keys: &[],
        },
        // 9. DNS Secret happy path + account happy path → True/ResolvedRefs.
        //    Reason must equal the literal "ResolvedRefs" constant (not just status==True).
        AcmeParseScenario {
            name: "dns_credential_happy_path_preserves_true",
            secrets: vec![
                (valid_account_key.clone(), valid_account_secret.clone()),
                make_secret("default", "cf-secret", &[("api-token", b"cf-token-value")]),
            ],
            acme: make_acme_dns01("default", "my-acme", "acme-account", "cloudflare", "cf-secret", None),
            expected_status: "True",
            expected_reason: Some("ResolvedRefs"),
            expected_message_substrings: &[],
            expected_refs_owner_for_keys: &[],
        },
        // 10. DNS Ok but account pending (NeedsRegistration) → Unknown survives.
        //     DNS success must NOT override account's soft Unknown state.
        AcmeParseScenario {
            name: "dns_credential_ok_preserves_account_needs_registration",
            secrets: vec![make_secret("default", "cf-secret", &[("api-token", b"cf-token")])],
            acme: make_acme_dns01("default", "my-acme", "missing-account", "cloudflare", "cf-secret", None),
            expected_status: "Unknown",
            expected_reason: Some("NeedsRegistration"),
            expected_message_substrings: &[],
            expected_refs_owner_for_keys: &[],
        },
        // 11. Account pending + DNS missing → DNS False overrides account's soft Unknown.
        //     Hard failure (False) beats soft pending (Unknown).
        AcmeParseScenario {
            name: "dns_credential_false_overrides_account_needs_registration",
            secrets: vec![],
            acme: make_acme_dns01(
                "default",
                "my-acme",
                "missing-account",
                "cloudflare",
                "missing-dns",
                None,
            ),
            expected_status: "False",
            expected_reason: Some("SecretNotFound"),
            expected_message_substrings: &["DNS credential"],
            expected_refs_owner_for_keys: &[],
        },
        // 12. Cross-namespace DNS credential without ReferenceGrant →
        //     ResolvedRefs=False/RefNotPermitted; message must name the target ns.
        AcmeParseScenario {
            name: "dns_credential_cross_namespace_denied",
            secrets: vec![
                (valid_account_key.clone(), valid_account_secret.clone()),
                make_secret("credentials", "cf-secret", &[("api-token", b"cf-token")]),
            ],
            acme: make_acme_dns01(
                "default",
                "my-acme",
                "acme-account",
                "cloudflare",
                "cf-secret",
                Some("credentials"),
            ),
            expected_status: "False",
            expected_reason: Some("RefNotPermitted"),
            expected_message_substrings: &["credentials"],
            expected_refs_owner_for_keys: &[],
        },
        // 13. Unknown DNS provider → DNS resolve branch is a no-op
        //     (validate() is the canonical error path).
        //     Account's True must survive. Reason is not checked (None).
        AcmeParseScenario {
            name: "dns_credential_unknown_provider_skips_dns_resolve",
            secrets: vec![(valid_account_key.clone(), valid_account_secret.clone())],
            acme: make_acme_dns01(
                "default",
                "my-acme",
                "acme-account",
                "route53-not-supported",
                "any-secret",
                None,
            ),
            expected_status: "True",
            expected_reason: None,
            expected_message_substrings: &[],
            expected_refs_owner_for_keys: &[],
        },
    ];

    for scenario in scenarios {
        run_acme_parse_scenario(scenario).await;
    }
}

// ========================================================================
// Test: on_delete clears SecretRefManager
// ========================================================================

#[tokio::test]
async fn test_on_delete_clears_refs() {
    // `parse` below reaches `notify_resource_changed` -> `get_sender()`; serialize
    // against the ACME service tests that install a handle (see `ACME_TEST_MUTEX`).
    let _acme_guard = crate::services::acme::service::ACME_TEST_MUTEX.lock().await;

    let (key, secret) = make_secret("default", "acme-account", &[("accountCredentials", br#"{"ok":true}"#)]);
    let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
    let handler = EdgionAcmeHandler::new();
    let acme = make_acme("default", "my-acme", "acme-account");

    // Parse to register reference
    let result = handler.parse(acme.clone(), &ctx).await;
    assert!(matches!(result, ProcessResult::Continue(_)));

    let owner = ResourceRef::new(
        ResourceKind::EdgionAcme,
        Some("default".to_string()),
        "my-acme".to_string(),
    );
    let refs = ctx.secret_ref_manager().get_refs("default/acme-account");
    assert!(refs.contains(&owner), "reference should be registered after parse");

    // Delete
    handler.on_delete(&acme, &ctx).await;

    let refs = ctx.secret_ref_manager().get_refs("default/acme-account");
    assert!(!refs.contains(&owner), "reference should be cleared after on_delete");
}

#[tokio::test]
async fn test_validate_rejects_invalid_gep2257_durations() {
    let handler = EdgionAcmeHandler::new();
    let (ctx, _runtime) = make_ctx(HashMap::new());

    let mut acme = make_acme_dns01("default", "my-acme", "acme-account", "cloudflare", "cf-token", None);
    assert!(
        handler.validate(&acme, &ctx).is_empty(),
        "baseline dns01 resource must validate cleanly"
    );

    // Bare integers (the pre-GEP-2257 form) must be rejected on every duration field.
    acme.spec.renewal.renew_before = "30".to_string();
    acme.spec.renewal.check_interval = "86400".to_string();
    acme.spec.renewal.fail_backoff = "bogus".to_string();
    if let AcmeChallenge::Dns01 {
        propagation_timeout,
        propagation_check_interval,
        ..
    } = &mut acme.spec.challenge
    {
        *propagation_timeout = "120".to_string();
        *propagation_check_interval = "5".to_string();
    }
    let errors = handler.validate(&acme, &ctx);
    for field in [
        "renewal.renewBefore",
        "renewal.checkInterval",
        "renewal.failBackoff",
        "challenge.propagationTimeout",
        "challenge.propagationCheckInterval",
    ] {
        assert!(
            errors.iter().any(|e| e.contains(field)),
            "expected a validation error for {field}, got: {errors:?}"
        );
    }
}

fn make_acme_dns01(
    ns: &str,
    name: &str,
    account_secret: &str,
    dns_provider: &str,
    dns_secret_name: &str,
    dns_secret_ns: Option<&str>,
) -> EdgionAcme {
    EdgionAcme {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(ns.to_string()),
            generation: Some(1),
            ..Default::default()
        },
        spec: edgion_resources::resources::edgion_acme::EdgionAcmeSpec {
            server: "https://acme-v02.api.letsencrypt.org/directory".to_string(),
            email: "admin@example.com".to_string(),
            private_key_secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: account_secret.to_string(),
                namespace: None,
            },
            domains: vec!["*.example.com".to_string()],
            key_type: Default::default(),
            challenge: AcmeChallenge::Dns01 {
                provider: dns_provider.to_string(),
                credential_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: dns_secret_name.to_string(),
                    namespace: dns_secret_ns.map(|s| s.to_string()),
                },
                propagation_timeout: "120s".to_string(),
                propagation_check_interval: "5s".to_string(),
            },
            renewal: Default::default(),
            external_account_binding: None,
            storage: AcmeStorageConfig {
                secret_name: "cert".to_string(),
                secret_namespace: None,
            },
            auto_edgion_tls: Default::default(),
        },
        status: None,
    }
}

// ========================================================================
// outcome_to_resolved_refs_action: table-driven tests
// ========================================================================

/// Expected outcome shape for a single scenario row.
enum ExpectedAction {
    /// AcmeResolveAction::NoAction
    NoAction,
    /// AcmeResolveAction::Continue with the given field expectations
    Continue {
        status: &'static str,
        reason: &'static str,
        message_substrings: &'static [&'static str],
    },
    /// AcmeResolveAction::Abort with the given field expectations
    Abort {
        status: &'static str,
        reason: &'static str,
        message_substrings: &'static [&'static str],
    },
}

struct OutcomeScenario {
    name: &'static str,
    outcome: ResolveOutcome<Secret>,
    ctx: AcmeResolveCtx<'static>,
    generation: Option<i64>,
    expected: ExpectedAction,
}

fn run_outcome_scenario(scenario: OutcomeScenario) {
    let action = outcome_to_resolved_refs_action(&scenario.outcome, "default/acme", scenario.generation, &scenario.ctx);
    match (&scenario.expected, action) {
        (ExpectedAction::NoAction, AcmeResolveAction::NoAction) => {}
        (
            ExpectedAction::Continue {
                status,
                reason,
                message_substrings,
            },
            AcmeResolveAction::Continue(cond),
        ) => {
            assert_eq!(cond.status, *status, "[{}] status mismatch", scenario.name);
            assert_eq!(cond.reason, *reason, "[{}] reason mismatch", scenario.name);
            for &substr in *message_substrings {
                assert!(
                    cond.message.contains(substr),
                    "[{}] message missing {:?}, got: {}",
                    scenario.name,
                    substr,
                    cond.message
                );
            }
            assert_eq!(
                cond.observed_generation, scenario.generation,
                "[{}] generation mismatch",
                scenario.name
            );
        }
        (
            ExpectedAction::Abort {
                status,
                reason,
                message_substrings,
            },
            AcmeResolveAction::Abort(cond),
        ) => {
            assert_eq!(cond.status, *status, "[{}] status mismatch", scenario.name);
            assert_eq!(cond.reason, *reason, "[{}] reason mismatch", scenario.name);
            for &substr in *message_substrings {
                assert!(
                    cond.message.contains(substr),
                    "[{}] message missing {:?}, got: {}",
                    scenario.name,
                    substr,
                    cond.message
                );
            }
            assert_eq!(
                cond.observed_generation, scenario.generation,
                "[{}] generation mismatch",
                scenario.name
            );
        }
        (expected, got) => {
            let expected_label = match expected {
                ExpectedAction::NoAction => "NoAction",
                ExpectedAction::Continue { .. } => "Continue",
                ExpectedAction::Abort { .. } => "Abort",
            };
            panic!("[{}] expected {}, got {:?}", scenario.name, expected_label, got);
        }
    }
}

#[test]
fn test_account_outcome_scenarios() {
    let scenarios: Vec<OutcomeScenario> = vec![
        OutcomeScenario {
            name: "account_ok_returns_continue_with_true_resolved_refs",
            outcome: ResolveOutcome::Ok(Secret::default()),
            ctx: AcmeResolveCtx::Account,
            generation: Some(1),
            expected: ExpectedAction::Continue {
                status: "True",
                reason: condition_reasons::RESOLVED_REFS,
                message_substrings: &["ACME account credentials loaded"],
            },
        },
        OutcomeScenario {
            name: "account_secret_not_found_returns_continue_unknown_needs_registration",
            outcome: ResolveOutcome::Pending(PendingReason::SecretNotFound {
                namespace: "default".to_string(),
                name: "acme-account".to_string(),
            }),
            ctx: AcmeResolveCtx::Account,
            generation: Some(2),
            expected: ExpectedAction::Continue {
                status: "Unknown",
                reason: acme_reasons::NEEDS_REGISTRATION,
                message_substrings: &["Account Secret absent"],
            },
        },
        OutcomeScenario {
            name: "account_ref_grant_missing_returns_continue_false_ref_not_permitted",
            outcome: ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing {
                from_ns: "default".to_string(),
                to_ns: "secrets".to_string(),
                secret_name: "acme-key".to_string(),
            }),
            ctx: AcmeResolveCtx::Account,
            generation: Some(3),
            expected: ExpectedAction::Continue {
                status: "False",
                reason: condition_reasons::REF_NOT_PERMITTED,
                message_substrings: &["Reference from default/EdgionAcme to Secret \"secrets/acme-key\""],
            },
        },
        OutcomeScenario {
            name: "account_missing_keys_returns_continue_unknown_needs_registration",
            // The Account MissingKeys arm uses a fixed bootstrap-state message
            // (NeedsRegistration) rather than interpolating the `keys` vec.
            outcome: ResolveOutcome::Invalid(InvalidCredentialError::MissingKeys(vec!["accountCredentials"])),
            ctx: AcmeResolveCtx::Account,
            generation: Some(4),
            expected: ExpectedAction::Continue {
                status: "Unknown",
                reason: acme_reasons::NEEDS_REGISTRATION,
                message_substrings: &["Account Secret absent"],
            },
        },
        OutcomeScenario {
            name: "account_decode_error_returns_abort_false_decode_error",
            outcome: ResolveOutcome::Invalid(InvalidCredentialError::DecodeError {
                key: "accountCredentials",
                detail: "invalid utf-8 at byte 3".to_string(),
            }),
            ctx: AcmeResolveCtx::Account,
            generation: Some(5),
            expected: ExpectedAction::Abort {
                status: "False",
                reason: condition_reasons::DECODE_ERROR,
                message_substrings: &["credential decode failed for key 'accountCredentials'", "invalid utf-8"],
            },
        },
        OutcomeScenario {
            name: "account_invalid_secret_type_returns_abort_false_invalid_type",
            outcome: ResolveOutcome::Invalid(InvalidCredentialError::InvalidSecretType {
                got: "Opaque".to_string(),
                allowed: &["edgion.io/acme-account"],
            }),
            ctx: AcmeResolveCtx::Account,
            generation: Some(6),
            expected: ExpectedAction::Abort {
                status: "False",
                reason: condition_reasons::INVALID_SECRET_TYPE,
                message_substrings: &["Secret.type", "Opaque", "not accepted for this credential"],
            },
        },
    ];

    for scenario in scenarios {
        run_outcome_scenario(scenario);
    }
}

#[test]
fn test_dns_outcome_scenarios() {
    let scenarios: Vec<OutcomeScenario> = vec![
        OutcomeScenario {
            name: "dns_ok_returns_no_action",
            outcome: ResolveOutcome::Ok(Secret::default()),
            ctx: AcmeResolveCtx::Dns { provider: "cloudflare" },
            generation: Some(1),
            expected: ExpectedAction::NoAction,
        },
        OutcomeScenario {
            name: "dns_secret_not_found_returns_continue_false_secret_not_found",
            outcome: ResolveOutcome::Pending(PendingReason::SecretNotFound {
                namespace: "default".to_string(),
                name: "cf-token".to_string(),
            }),
            ctx: AcmeResolveCtx::Dns { provider: "cloudflare" },
            generation: Some(2),
            expected: ExpectedAction::Continue {
                status: "False",
                reason: condition_reasons::SECRET_NOT_FOUND,
                message_substrings: &["DNS credential Secret 'default/cf-token' not found"],
            },
        },
        OutcomeScenario {
            name: "dns_ref_grant_missing_returns_continue_false_ref_not_permitted_with_dns_label",
            outcome: ResolveOutcome::Pending(PendingReason::ReferenceGrantMissing {
                from_ns: "default".to_string(),
                to_ns: "secrets".to_string(),
                secret_name: "cf-token".to_string(),
            }),
            ctx: AcmeResolveCtx::Dns { provider: "cloudflare" },
            generation: Some(3),
            expected: ExpectedAction::Continue {
                status: "False",
                reason: condition_reasons::REF_NOT_PERMITTED,
                message_substrings: &[
                    "Reference from default/EdgionAcme to DNS credential Secret \"secrets/cf-token\"",
                ],
            },
        },
        OutcomeScenario {
            name: "dns_missing_keys_returns_continue_false_missing_keys_with_provider",
            outcome: ResolveOutcome::Invalid(InvalidCredentialError::MissingKeys(vec!["api-token"])),
            ctx: AcmeResolveCtx::Dns { provider: "cloudflare" },
            generation: Some(4),
            expected: ExpectedAction::Continue {
                status: "False",
                reason: condition_reasons::MISSING_KEYS,
                message_substrings: &["DNS credential Secret for provider 'cloudflare'", "api-token"],
            },
        },
        OutcomeScenario {
            name: "dns_decode_error_returns_continue_false_decode_error_with_dns_label",
            outcome: ResolveOutcome::Invalid(InvalidCredentialError::DecodeError {
                key: "api-token",
                detail: "invalid utf-8 at byte 1".to_string(),
            }),
            ctx: AcmeResolveCtx::Dns { provider: "cloudflare" },
            generation: Some(5),
            expected: ExpectedAction::Continue {
                status: "False",
                reason: condition_reasons::DECODE_ERROR,
                message_substrings: &["DNS credential decode failed for key 'api-token'", "invalid utf-8"],
            },
        },
        OutcomeScenario {
            name: "dns_invalid_secret_type_returns_continue_false_invalid_type_with_dns_label",
            outcome: ResolveOutcome::Invalid(InvalidCredentialError::InvalidSecretType {
                got: "kubernetes.io/tls".to_string(),
                allowed: &["Opaque"],
            }),
            ctx: AcmeResolveCtx::Dns { provider: "cloudflare" },
            generation: Some(6),
            expected: ExpectedAction::Continue {
                status: "False",
                reason: condition_reasons::INVALID_SECRET_TYPE,
                message_substrings: &["DNS credential Secret.type", "kubernetes.io/tls", "not accepted"],
            },
        },
    ];

    for scenario in scenarios {
        run_outcome_scenario(scenario);
    }
}
