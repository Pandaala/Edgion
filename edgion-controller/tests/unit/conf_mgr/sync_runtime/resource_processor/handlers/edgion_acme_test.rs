//! Unit tests for `edgion_acme.rs` (see `super`).
use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::handlers::GatewayHandler;
use crate::conf_mgr::sync_runtime::resource_processor::{ProcessorHandler, ResourceProcessor};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use crate::services::acme::service::{NotificationCapture, ACME_TEST_MUTEX};
use edgion_resources::prelude_resources::Gateway;
use edgion_resources::resources::common::ParentReference;
use edgion_resources::resources::common::SecretObjectReference;
use edgion_resources::resources::edgion_acme::{
    AcmeCertPhase, AcmeChallenge, AcmeExternalAccountBinding, AcmeStorageConfig, ActiveHttpChallenge, EdgionAcmeStatus,
};
use edgion_resources::resources::edgion_tls::{GatewayAttachmentProof, ResolvedListenerAttachment};
use edgion_resources::resources::gateway::{GatewaySpec, GatewayStatus, Listener, ListenerStatus};
use edgion_resources::resources::{ReferenceGrant, ReferenceGrantFrom, ReferenceGrantSpec, ReferenceGrantTo};
use edgion_resources::ListenerKey;
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
    let gateway_processor = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        16,
        Arc::new(NoopGatewayHandler),
        runtime.clone(),
    ));
    gateway_processor.save(make_gateway("default", "gw", vec![listener("http", 80, "HTTP")]), None);
    runtime.processor_registry.register(gateway_processor);
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    (ctx, runtime)
}

struct NoopGatewayHandler;

#[async_trait::async_trait]
impl ProcessorHandler<Gateway> for NoopGatewayHandler {}

fn listener(name: &str, port: i32, protocol: &str) -> Listener {
    Listener {
        name: name.to_string(),
        hostname: None,
        port,
        protocol: protocol.to_string(),
        tls: None,
        allowed_routes: None,
    }
}

fn make_gateway(namespace: &str, name: &str, listeners: Vec<Listener>) -> Gateway {
    let current_status = GatewayStatus {
        listeners: Some(
            listeners
                .iter()
                .map(|listener| ListenerStatus {
                    name: listener.name.clone(),
                    supported_kinds: vec![],
                    attached_routes: 0,
                    conditions: vec![],
                })
                .collect(),
        ),
        ..Default::default()
    };
    Gateway {
        metadata: ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            generation: Some(1),
            ..Default::default()
        },
        spec: GatewaySpec {
            current_status: Some(current_status),
            gateway_class_name: "edgion".to_string(),
            listeners: Some(listeners),
            addresses: None,
            tls: None,
            allowed_listeners: None,
            infrastructure: None,
            default_scope: None,
            resolved_inbound_proxy_protocol: Default::default(),
            resolved_attachment_proof: Some(GatewayAttachmentProof {
                instance: "test-instance".to_string(),
                generation: 1,
                epoch: "1".to_string(),
                attachment_digest: "a".repeat(64),
            }),
        },
        status: None,
    }
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
            current_status: None,
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
            resolved_listener_attachments: None,
            notify_after_publish: false,
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

/// Exercise the real handler while checking publication before its notification hook.
struct PublicationCheckedAcmeHandler;

#[async_trait::async_trait]
impl ProcessorHandler<EdgionAcme> for PublicationCheckedAcmeHandler {
    async fn parse(&self, acme: EdgionAcme, ctx: &HandlerContext) -> ProcessResult<EdgionAcme> {
        EdgionAcmeHandler::new().parse(acme, ctx).await
    }

    fn update_status(&self, acme: &mut EdgionAcme, ctx: &HandlerContext, errors: &[String]) {
        EdgionAcmeHandler::new().update_status(acme, ctx, errors);
    }

    async fn after_publish(&self, acme: &EdgionAcme, ctx: &HandlerContext) {
        let key = format!(
            "{}/{}",
            acme.metadata.namespace.as_deref().unwrap(),
            acme.metadata.name.as_deref().unwrap()
        );
        let cached = ctx
            .runtime()
            .processor_registry
            .get("EdgionAcme")
            .unwrap()
            .get_cached_any(&key)
            .unwrap()
            .downcast::<EdgionAcme>()
            .unwrap();
        assert_eq!(cached.metadata.uid, acme.metadata.uid);
        assert_eq!(cached.metadata.generation, acme.metadata.generation);
        assert_eq!(
            resolved_refs_identity(cached.spec.current_status.as_ref()),
            resolved_refs_identity(acme.spec.current_status.as_ref())
        );
        assert!(cached
            .status
            .as_ref()
            .unwrap()
            .conditions
            .iter()
            .any(|condition| condition.type_ == "Accepted"));
        EdgionAcmeHandler::new().after_publish(acme, ctx).await;
    }
}

#[tokio::test]
async fn status_echoes_with_unpersisted_conditions_do_not_notify_again() {
    let _guard = ACME_TEST_MUTEX.lock().await;
    let mut notifications = NotificationCapture::install();
    let (ctx, runtime) = make_ctx(HashMap::new());
    let processor = Arc::new(ResourceProcessor::<EdgionAcme>::new(
        "EdgionAcme",
        16,
        Arc::new(PublicationCheckedAcmeHandler),
        runtime.clone(),
    ));
    runtime.processor_registry.register(processor.clone());
    let mut raw = make_acme("default", "echo", "account");
    raw.metadata.uid = Some("uid-one".to_string());
    // parse may decide to notify, but must not send before publication.
    let parsed = match EdgionAcmeHandler::new().parse(raw.clone(), &ctx).await {
        ProcessResult::Continue(parsed) => parsed,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };
    assert!(parsed.spec.notify_after_publish);
    assert!(notifications.drain().is_empty());
    processor.on_init_apply(raw.clone(), None).await;
    assert_eq!(notifications.drain(), ["default/echo"]);

    let gateway_processor = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        16,
        Arc::new(NoopGatewayHandler),
        runtime.clone(),
    ));
    let mut gateway = make_gateway("default", "gw", vec![listener("http", 80, "HTTP")]);
    gateway.spec.resolved_attachment_proof.as_mut().unwrap().epoch = "new-publication".to_string();
    gateway_processor.save(gateway, None);
    runtime.processor_registry.register(gateway_processor);

    // Deliberately never feed the computed conditions back into raw, as when
    // a service lifecycle write wins before each generic status CAS attempt.
    for iteration in 0..20 {
        raw.metadata.resource_version = Some(format!("service-{iteration}"));
        raw.status = Some(EdgionAcmeStatus {
            phase: if iteration % 2 == 0 {
                AcmeCertPhase::Issuing
            } else {
                AcmeCertPhase::Failed
            },
            active_challenges: if iteration % 2 == 0 {
                vec![ActiveHttpChallenge {
                    domain: "example.com".to_string(),
                    token: "token".to_string(),
                    key_authorization: "authorization".to_string(),
                    expire_at: 100,
                }]
            } else {
                vec![]
            },
            ..Default::default()
        });
        raw.spec.notify_after_publish = true; // A reused clone cannot replay old intent.
        processor
            .process_work_item("default/echo", Some(raw.clone()), None, TriggerChain::default())
            .await;
        if iteration == 0 {
            assert_eq!(notifications.drain(), ["default/echo"]);
        } else {
            assert!(notifications.drain().is_empty(), "status echo {iteration} notified");
        }
        assert_eq!(
            processor.get("default/echo").unwrap().spec.notify_after_publish,
            iteration == 0
        );
    }
    raw.metadata.generation = Some(2);
    processor
        .process_work_item("default/echo", Some(raw.clone()), None, TriggerChain::default())
        .await;
    assert_eq!(notifications.drain(), ["default/echo"]);
    raw.metadata.uid = Some("uid-two".to_string());
    processor
        .process_work_item("default/echo", Some(raw.clone()), None, TriggerChain::default())
        .await;
    assert_eq!(notifications.drain(), ["default/echo"]);
    raw.metadata.generation = None;
    processor
        .process_work_item("default/echo", Some(raw.clone()), None, TriggerChain::default())
        .await;
    assert_eq!(notifications.drain(), ["default/echo"]);
    processor
        .process_work_item("default/echo", Some(raw), None, TriggerChain::default())
        .await;
    assert!(
        notifications.drain().is_empty(),
        "missing generation is not always a first create"
    );
}

#[tokio::test]
async fn recovered_credentials_are_published_before_the_single_notification() {
    let _guard = ACME_TEST_MUTEX.lock().await;
    let mut notifications = NotificationCapture::install();
    let (secret_key, valid) = make_secret("default", "account", &[("accountCredentials", br#"{"valid":true}"#)]);
    let (_, runtime) = make_ctx(HashMap::from([(secret_key.clone(), valid.clone())]));
    let processor = Arc::new(ResourceProcessor::<EdgionAcme>::new(
        "EdgionAcme",
        16,
        Arc::new(PublicationCheckedAcmeHandler),
        runtime.clone(),
    ));
    runtime.processor_registry.register(processor.clone());
    let mut raw = make_acme("default", "recovery", "account");
    processor.on_init_apply(raw.clone(), None).await;
    assert_eq!(notifications.drain(), ["default/recovery"]);
    let (_, corrupted) = make_secret("default", "account", &[("accountCredentials", b"invalid-json")]);
    runtime
        .secret_store
        .replace_all(HashMap::from([(secret_key.clone(), corrupted)]));
    raw.spec.notify_after_publish = true;
    processor
        .process_work_item("default/recovery", Some(raw.clone()), None, TriggerChain::default())
        .await;
    assert_eq!(notifications.drain(), ["default/recovery"]);
    assert_eq!(
        resolved_refs_identity(processor.get("default/recovery").unwrap().spec.current_status.as_ref())
            .unwrap()
            .0,
        "False"
    );
    assert!(processor.get("default/recovery").unwrap().spec.notify_after_publish);
    runtime.secret_store.replace_all(HashMap::from([(secret_key, valid)]));
    processor
        .process_work_item("default/recovery", Some(raw.clone()), None, TriggerChain::default())
        .await;
    assert_eq!(notifications.drain(), ["default/recovery"]);
    assert_eq!(
        resolved_refs_identity(processor.get("default/recovery").unwrap().spec.current_status.as_ref())
            .unwrap()
            .0,
        "True"
    );
    processor
        .process_work_item("default/recovery", Some(raw), None, TriggerChain::default())
        .await;
    assert!(notifications.drain().is_empty());
}

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
    // Keep these handler scenarios serialized with notification-capture tests;
    // only after_publish, not parse, now accesses the global service sender.
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
// Test: Secret reference target metadata
// ========================================================================

#[tokio::test]
async fn secret_reference_targets_use_gateway_api_defaults_and_reject_other_types() {
    let _guard = ACME_TEST_MUTEX.lock().await;
    let handler = EdgionAcmeHandler::new();
    for field in ["privateKeySecretRef", "externalAccountBinding.keySecretRef"] {
        for (group, kind, valid) in [
            (None, None, true),
            (Some(""), None, true),
            (None, Some("Secret"), true),
            (Some(""), Some("Secret"), true),
            (Some("core"), Some("Secret"), false),
            (Some("example.io"), Some("Secret"), false),
            (None, Some("ConfigMap"), false),
            (None, Some("secret"), false),
            (None, Some(""), false),
        ] {
            for secret_present in [false, true] {
                let secrets = if secret_present {
                    HashMap::from([make_secret(
                        "default",
                        "account",
                        &[("accountCredentials", br#"{"ok":true}"#)],
                    )])
                } else {
                    HashMap::new()
                };
                let (ctx, _) = make_ctx(secrets);
                let mut input = make_acme("default", "target", "account");
                input.spec.external_account_binding = Some(AcmeExternalAccountBinding {
                    key_id: "public-id".to_string(),
                    key_secret_ref: SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "eab".to_string(),
                        namespace: None,
                    },
                });
                let reference = if field == "privateKeySecretRef" {
                    &mut input.spec.private_key_secret_ref
                } else {
                    &mut input.spec.external_account_binding.as_mut().unwrap().key_secret_ref
                };
                reference.group = group.map(str::to_string);
                reference.kind = kind.map(str::to_string);
                // Exercise the same typed input after YAML and JSON decode (FS/etcd
                // and Kubernetes inputs converge on this shared handler).
                let input: EdgionAcme = serde_yaml::from_str(&serde_yaml::to_string(&input).unwrap()).unwrap();
                let input: EdgionAcme = serde_json::from_value(serde_json::to_value(input).unwrap()).unwrap();
                let errors = handler.validate(&input, &ctx);
                assert_eq!(errors.is_empty(), valid, "{field}: {group:?}/{kind:?}");
                let ProcessResult::Continue(mut parsed) = handler.parse(input, &ctx).await else {
                    panic!("invalid references must retain status publication");
                };
                handler.update_status(&mut parsed, &ctx, &errors);
                let conditions = &parsed.status.as_ref().unwrap().conditions;
                let resolved = conditions
                    .iter()
                    .find(|condition| condition.type_ == "ResolvedRefs")
                    .unwrap();
                let accepted = conditions
                    .iter()
                    .find(|condition| condition.type_ == "Accepted")
                    .unwrap();
                assert_eq!(accepted.status, if valid { "True" } else { "False" });
                if valid {
                    assert_eq!(resolved.status, if secret_present { "True" } else { "Unknown" });
                    assert!(!ctx.secret_ref_manager().get_refs("default/account").is_empty());
                } else {
                    assert_eq!(resolved.status, "False");
                    assert_eq!(resolved.reason, "InvalidKind");
                    assert_eq!(resolved.observed_generation, Some(1));
                    assert!(resolved.message.starts_with(field));
                    assert!(ctx.secret_ref_manager().get_refs("default/account").is_empty());
                    assert!(ctx.secret_ref_manager().get_refs("default/eab").is_empty());
                }
            }
        }
    }
}

#[tokio::test]
async fn invalid_target_clears_old_secret_edges_and_recovers_on_spec_update() {
    let _guard = ACME_TEST_MUTEX.lock().await;
    let mut notifications = NotificationCapture::install();
    let (_, runtime) = make_ctx(HashMap::new());
    let processor = Arc::new(ResourceProcessor::<EdgionAcme>::new(
        "EdgionAcme",
        16,
        Arc::new(EdgionAcmeHandler::new()),
        runtime.clone(),
    ));
    runtime.processor_registry.register(processor.clone());
    let mut raw = make_acme("default", "target-recovery", "account");
    let owner = ResourceRef::new(
        ResourceKind::EdgionAcme,
        Some("default".to_string()),
        "target-recovery".to_string(),
    );
    processor.on_init_apply(raw.clone(), None).await;
    assert!(runtime.secret_ref_manager.get_refs("default/account").contains(&owner));
    assert_eq!(notifications.drain(), ["default/target-recovery"]);
    runtime.cross_ns_ref_mgr.add_ref("other".to_string(), owner.clone());
    raw.metadata.generation = Some(2);
    raw.spec.private_key_secret_ref.group = Some("example.io".to_string());
    raw.spec.private_key_secret_ref.namespace = Some("other".to_string());
    processor
        .process_work_item(
            "default/target-recovery",
            Some(raw.clone()),
            None,
            TriggerChain::default(),
        )
        .await;
    let denied = processor.get("default/target-recovery").unwrap();
    assert_eq!(
        resolved_refs_identity(denied.spec.current_status.as_ref()),
        Some(("False", "InvalidKind"))
    );
    assert!(!runtime.secret_ref_manager.get_refs("default/account").contains(&owner));
    assert!(!runtime.secret_ref_manager.get_refs("other/account").contains(&owner));
    assert!(!runtime.cross_ns_ref_mgr.get_refs("other").contains(&owner));
    assert_eq!(notifications.drain(), ["default/target-recovery"]);
    raw.metadata.generation = Some(3);
    raw.spec.private_key_secret_ref.group = Some("".to_string());
    raw.spec.private_key_secret_ref.namespace = None;
    processor
        .process_work_item("default/target-recovery", Some(raw), None, TriggerChain::default())
        .await;
    let recovered = processor.get("default/target-recovery").unwrap();
    assert_eq!(
        resolved_refs_identity(recovered.spec.current_status.as_ref()),
        Some(("Unknown", "NeedsRegistration"))
    );
    assert!(runtime.secret_ref_manager.get_refs("default/account").contains(&owner));
    assert_eq!(notifications.drain(), ["default/target-recovery"]);
}

// ========================================================================
// Test: on_delete clears SecretRefManager
// ========================================================================

#[tokio::test]
async fn test_on_delete_clears_refs() {
    // Share the handler scenario lock with tests that exercise after_publish.
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
    ctx.runtime()
        .cross_ns_ref_mgr
        .add_ref("other".to_string(), owner.clone());
    assert_eq!(
        ctx.runtime().gateway_route_index.get_routes_for_gateway("default/gw"),
        vec![(ResourceKind::EdgionAcme, "default/my-acme".to_string())]
    );

    // Delete
    handler.on_delete(&acme, &ctx).await;

    let refs = ctx.secret_ref_manager().get_refs("default/acme-account");
    assert!(!refs.contains(&owner), "reference should be cleared after on_delete");
    assert!(!ctx.runtime().cross_ns_ref_mgr.get_refs("other").contains(&owner));
    assert!(ctx
        .runtime()
        .gateway_route_index
        .get_routes_for_gateway("default/gw")
        .is_empty());
}

#[tokio::test]
async fn test_parse_resolves_only_selected_http_listener() {
    let _acme_guard = crate::services::acme::service::ACME_TEST_MUTEX.lock().await;
    let (key, secret) = make_secret("default", "acme-account", &[("accountCredentials", br#"{"ok":true}"#)]);
    let (ctx, runtime) = make_ctx(HashMap::from([(key, secret)]));
    let gateway_processor = runtime
        .processor_registry
        .get("Gateway")
        .expect("Gateway processor must exist");
    let gateway = make_gateway(
        "default",
        "gw",
        vec![listener("http", 80, "HTTP"), listener("https", 443, "HTTPS")],
    );
    gateway_processor
        .get_cached_any("default/gw")
        .expect("baseline Gateway must exist");
    // Replace the typed processor so the authoritative cache carries both listeners.
    let replacement = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        16,
        Arc::new(NoopGatewayHandler),
        runtime.clone(),
    ));
    replacement.save(gateway, None);
    runtime.processor_registry.register(replacement);

    let mut acme = make_acme("default", "my-acme", "acme-account");
    let AcmeChallenge::Http01 { gateway_ref } = &mut acme.spec.challenge;
    gateway_ref.section_name = Some("http".to_string());
    gateway_ref.port = Some(80);
    let parsed = match EdgionAcmeHandler::new().parse(acme, &ctx).await {
        ProcessResult::Continue(parsed) => parsed,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };
    let attachments = parsed
        .spec
        .resolved_listener_attachments
        .expect("selected HTTP listener must resolve");
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].listener_name, "http");

    let mut https_only = make_acme("default", "https-only", "acme-account");
    let AcmeChallenge::Http01 { gateway_ref } = &mut https_only.spec.challenge;
    gateway_ref.section_name = Some("https".to_string());
    let parsed = match EdgionAcmeHandler::new().parse(https_only, &ctx).await {
        ProcessResult::Continue(parsed) => parsed,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };
    assert!(parsed.spec.resolved_listener_attachments.is_none());
    let resolved = parsed
        .status
        .expect("status must exist")
        .conditions
        .into_iter()
        .find(|condition| condition.type_ == "ResolvedRefs")
        .expect("ResolvedRefs must exist");
    assert_eq!(resolved.status, "False");
    assert_eq!(resolved.reason, condition_reasons::NO_MATCHING_PARENT);
}

#[tokio::test]
async fn test_parse_rejects_explicitly_ineligible_gateway_listeners() {
    let _acme_guard = crate::services::acme::service::ACME_TEST_MUTEX.lock().await;
    for condition in [
        condition_false("Programmed", "Pending", "not programmed", Some(1)),
        condition_false("Accepted", "Invalid", "not accepted", Some(1)),
        condition_true("Conflicted", "PortConflict", "conflicted", Some(1)),
    ] {
        let (ctx, runtime) = make_ctx(HashMap::new());
        let mut gateway = make_gateway("default", "gw", vec![listener("http", 80, "HTTP")]);
        gateway.spec.current_status = Some(GatewayStatus {
            addresses: None,
            conditions: None,
            listeners: Some(vec![ListenerStatus {
                name: "http".to_string(),
                supported_kinds: Vec::new(),
                attached_routes: 0,
                conditions: vec![condition],
            }]),
        });
        let replacement = Arc::new(ResourceProcessor::<Gateway>::new(
            "Gateway",
            16,
            Arc::new(NoopGatewayHandler),
            runtime.clone(),
        ));
        replacement.save(gateway, None);
        runtime.processor_registry.register(replacement);
        let parsed = match EdgionAcmeHandler::new()
            .parse(make_acme("default", "ownership", "account"), &ctx)
            .await
        {
            ProcessResult::Continue(parsed) => parsed,
            ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
        };
        assert!(parsed.spec.resolved_listener_attachments.is_none());
    }
}

#[tokio::test]
async fn test_cross_namespace_gateway_requires_exact_reference_grant() {
    let _acme_guard = crate::services::acme::service::ACME_TEST_MUTEX.lock().await;
    let (ctx, runtime) = make_ctx(HashMap::new());
    let replacement = Arc::new(ResourceProcessor::<Gateway>::new(
        "Gateway",
        16,
        Arc::new(NoopGatewayHandler),
        runtime.clone(),
    ));
    replacement.save(make_gateway("infra", "gw", vec![listener("http", 80, "HTTP")]), None);
    runtime.processor_registry.register(replacement);
    let mut resource = make_acme("apps", "ownership", "account");
    let AcmeChallenge::Http01 { gateway_ref } = &mut resource.spec.challenge;
    gateway_ref.namespace = Some("infra".to_string());

    let denied = match EdgionAcmeHandler::new().parse(resource.clone(), &ctx).await {
        ProcessResult::Continue(parsed) => parsed,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };
    assert!(denied.spec.resolved_listener_attachments.is_none());

    let grant: ReferenceGrant = ReferenceGrant {
        metadata: ObjectMeta {
            namespace: Some("infra".to_string()),
            name: Some("allow-acme".to_string()),
            ..Default::default()
        },
        spec: ReferenceGrantSpec {
            from: vec![ReferenceGrantFrom {
                group: "edgion.io".to_string(),
                kind: "EdgionAcme".to_string(),
                namespace: "apps".to_string(),
            }],
            to: vec![ReferenceGrantTo {
                group: "gateway.networking.k8s.io".to_string(),
                kind: "Gateway".to_string(),
                name: Some("gw".to_string()),
            }],
        },
    };
    runtime
        .ref_grant_store
        .upsert("infra/allow-acme".to_string(), Arc::new(grant));
    let allowed = match EdgionAcmeHandler::new().parse(resource, &ctx).await {
        ProcessResult::Continue(parsed) => parsed,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };
    assert_eq!(allowed.spec.resolved_listener_attachments.unwrap().len(), 1);
}

#[tokio::test]
async fn gateway_proof_publication_requeues_acme_consumers() {
    let runtime = Arc::new(ControllerRuntime::default());
    let acme_processor = Arc::new(ResourceProcessor::<EdgionAcme>::new(
        ResourceKind::EdgionAcme.as_str(),
        16,
        Arc::new(EdgionAcmeHandler::new()),
        runtime.clone(),
    ));
    runtime.processor_registry.register(acme_processor.clone());
    runtime.gateway_route_index.update_route(
        ResourceKind::EdgionAcme,
        "default/ownership",
        &[ParentReference {
            group: None,
            kind: None,
            namespace: None,
            name: "gw".to_string(),
            section_name: Some("http".to_string()),
            port: None,
        }],
        "default",
    );
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime);
    GatewayHandler::new(None, None)
        .after_publish(&make_gateway("default", "gw", vec![listener("http", 80, "HTTP")]), &ctx)
        .await;
    let queued = tokio::time::timeout(std::time::Duration::from_secs(1), acme_processor.workqueue().dequeue())
        .await
        .expect("proof consumer must be requeued")
        .expect("ACME workqueue must remain open");
    assert_eq!(queued.key, "default/ownership");
}

#[tokio::test]
async fn test_validate_rejects_invalid_gep2257_durations() {
    let handler = EdgionAcmeHandler::new();
    let (ctx, _runtime) = make_ctx(HashMap::new());

    let mut acme = make_acme("default", "my-acme", "acme-account");
    assert!(
        handler.validate(&acme, &ctx).is_empty(),
        "baseline http01 resource must validate cleanly"
    );

    // Bare integers (the pre-GEP-2257 form) must be rejected on every duration field.
    acme.spec.renewal.renew_before = "30".to_string();
    acme.spec.renewal.check_interval = "86400".to_string();
    acme.spec.renewal.fail_backoff = "bogus".to_string();
    let errors = handler.validate(&acme, &ctx);
    for field in ["renewal.renewBefore", "renewal.checkInterval", "renewal.failBackoff"] {
        assert!(
            errors.iter().any(|e| e.contains(field)),
            "expected a validation error for {field}, got: {errors:?}"
        );
    }
}

// ========================================================================
// outcome_to_resolved_refs_action: table-driven tests
// ========================================================================

/// Expected outcome shape for a single scenario row.
enum ExpectedAction {
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
    generation: Option<i64>,
    expected: ExpectedAction,
}

fn run_outcome_scenario(scenario: OutcomeScenario) {
    let action = outcome_to_resolved_refs_action(&scenario.outcome, "default/acme", scenario.generation);
    match (&scenario.expected, action) {
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

#[tokio::test]
async fn test_validate_rejects_wildcards() {
    let (ctx, _runtime) = make_ctx(HashMap::new());
    let mut acme = make_acme("default", "wildcard", "account");
    acme.spec.domains = vec!["*.example.com".to_string(), "example.com".to_string()];
    let errors = EdgionAcmeHandler::new().validate(&acme, &ctx);
    assert_eq!(
        errors,
        ["EdgionAcme: built-in HTTP-01 does not support wildcard domains; use an externally issued TLS Secret"]
    );
}

#[tokio::test]
async fn parse_unresolvable_gateway_ref_clears_stale_attachments_and_keeps_active_challenges() {
    let _acme_guard = crate::services::acme::service::ACME_TEST_MUTEX.lock().await;
    let (key, secret) = make_secret("default", "account", &[("accountCredentials", br#"{"ok":true}"#)]);
    let (ctx, _runtime) = make_ctx(HashMap::from([(key, secret)]));
    let mut acme = make_acme("default", "ownership", "account");
    acme.spec.notify_after_publish = true;
    let AcmeChallenge::Http01 { gateway_ref } = &mut acme.spec.challenge;
    gateway_ref.name = "missing".to_string();
    acme.spec.resolved_listener_attachments = Some(vec![ResolvedListenerAttachment {
        gateway_namespace: "default".to_string(),
        gateway_name: "gw".to_string(),
        listener_name: "http".to_string(),
        listener_key: ListenerKey("0.0.0.0:80/HTTP".to_string()),
        gateway_proof: GatewayAttachmentProof {
            instance: "test-instance".to_string(),
            generation: 1,
            epoch: "1".to_string(),
            attachment_digest: "a".repeat(64),
        },
    }]);
    acme.status = Some(EdgionAcmeStatus {
        active_challenges: vec![ActiveHttpChallenge {
            domain: "example.com".to_string(),
            token: "ownership-token".to_string(),
            key_authorization: "ownership-token.authorization".to_string(),
            expire_at: 4102444800,
        }],
        ..Default::default()
    });

    let parsed = match EdgionAcmeHandler::new().parse(acme, &ctx).await {
        ProcessResult::Continue(parsed) => parsed,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };
    assert!(parsed.spec.resolved_listener_attachments.is_none());
    assert!(parsed.spec.notify_after_publish);
    // A direct parse has not produced the current transport projection yet.
    assert!(parsed.active_challenges().is_empty());
    assert_eq!(parsed.status.as_ref().unwrap().active_challenges.len(), 1);
    assert_eq!(
        parsed.status.as_ref().unwrap().active_challenges[0].token,
        "ownership-token"
    );
    let resolved = parsed
        .status
        .as_ref()
        .expect("status must exist")
        .conditions
        .iter()
        .find(|condition| condition.type_ == condition_types::RESOLVED_REFS)
        .expect("ResolvedRefs must exist");
    assert_eq!(resolved.status, "False");
    assert_eq!(resolved.reason, condition_reasons::NO_MATCHING_PARENT);
}

#[tokio::test]
async fn actual_acme_fields_keep_eab_out_of_handler_dependencies_and_parent_defaults_canonical() {
    let _guard = ACME_TEST_MUTEX.lock().await;
    let (key, secret) = make_secret("default", "account", &[("accountCredentials", b"{}")]);
    let (ctx, runtime) = make_ctx(HashMap::from([(key, secret)]));
    let mut acme = make_acme("default", "issuer", "account");
    acme.spec.external_account_binding = Some(AcmeExternalAccountBinding {
        key_id: "id".into(),
        key_secret_ref: SecretObjectReference {
            name: "eab".into(),
            namespace: Some("other".into()),
            group: None,
            kind: None,
        },
    });
    let baseline = resolve_acme_listener_attachments(&acme, &runtime).unwrap();
    let AcmeChallenge::Http01 { gateway_ref } = &mut acme.spec.challenge;
    gateway_ref.group = Some("gateway.networking.k8s.io".into());
    gateway_ref.kind = Some("Gateway".into());
    gateway_ref.namespace = Some("default".into());
    assert_eq!(resolve_acme_listener_attachments(&acme, &runtime).unwrap(), baseline);
    let ProcessResult::Continue(parsed) = EdgionAcmeHandler::new().parse(acme.clone(), &ctx).await else {
        panic!("unexpected skip")
    };
    let owner = ResourceRef::new(ResourceKind::EdgionAcme, Some("default".into()), "issuer".into());
    assert_eq!(
        runtime.secret_ref_manager.get_dependencies(&owner.key()),
        ["default/account"]
    );
    assert!(runtime.cross_ns_ref_mgr.get_refs("other").is_empty());
    assert!(parsed
        .status
        .as_ref()
        .unwrap()
        .conditions
        .iter()
        .any(|c| c.type_ == "ResolvedRefs" && c.status == "True"));
    let AcmeChallenge::Http01 { gateway_ref } = &mut acme.spec.challenge;
    gateway_ref.group = Some(String::new());
    assert!(matches!(
        resolve_acme_listener_attachments(&acme, &runtime),
        Err(AcmeGatewayRefError::InvalidKind { .. })
    ));
    let AcmeChallenge::Http01 { gateway_ref } = &mut acme.spec.challenge;
    gateway_ref.group = None;
    gateway_ref.section_name = Some("http".into());
    gateway_ref.port = Some(81);
    assert!(matches!(
        resolve_acme_listener_attachments(&acme, &runtime),
        Err(AcmeGatewayRefError::NoMatchingListener)
    ));
}
