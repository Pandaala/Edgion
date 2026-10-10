use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use edgion_resources::prelude_resources::Gateway;
use edgion_resources::resources::common::ParentReference;
use edgion_resources::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode};
use edgion_resources::resources::gateway::{GatewaySpec, Listener as GatewayListener, SecretObjectReference};
use kube::api::ObjectMeta;
use std::sync::Arc;

fn make_ctx() -> HandlerContext {
    use crate::conf_mgr::ControllerRuntime;
    HandlerContext::new(
        None,
        None,
        TriggerChain::default(),
        3,
        20,
        Arc::new(ControllerRuntime::default()),
    )
}

struct NoopHandler;

#[async_trait::async_trait]
impl ProcessorHandler<Gateway> for NoopHandler {}

fn make_https_gateway(ns: &str, name: &str) -> Gateway {
    Gateway {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(ns.to_string()),
            ..Default::default()
        },
        spec: GatewaySpec {
            current_status: None,
            gateway_class_name: "test".to_string(),
            listeners: Some(vec![GatewayListener {
                name: "https".to_string(),
                hostname: None,
                port: 443,
                protocol: "HTTPS".to_string(),
                tls: None,
                allowed_routes: None,
            }]),
            addresses: None,
            tls: None,
            allowed_listeners: None,
            infrastructure: None,
            default_scope: None,
            resolved_inbound_proxy_protocol: Default::default(),
            resolved_attachment_proof: None,
        },
        status: None,
    }
}

fn make_ctx_with_gateways(gateways: &[(&str, &str)]) -> HandlerContext {
    use crate::conf_mgr::ControllerRuntime;

    let runtime = Arc::new(ControllerRuntime::default());
    let store = Arc::new(LocalRawStore::<Gateway>::new());

    for (ns, name) in gateways {
        let key = format!("{}/{}", ns, name);
        store.upsert(key, Arc::new(make_https_gateway(ns, name)));
    }

    // ResourceProcessor::new spawns a Tokio background task (Workqueue delay loop),
    // so we need a Tokio runtime for initialization even though the test itself is sync.
    // The spawned task is only needed for requeue operations; get_raw_any is sync-safe.
    let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let processor = rt.block_on(async {
        Arc::new(ResourceProcessor::<Gateway>::new(
            "Gateway",
            100,
            Arc::new(NoopHandler),
            runtime.clone(),
        ))
    });
    processor.set_raw_store(store);
    runtime.processor_registry.register(processor);

    HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime)
}

fn make_tls(parent_refs: Option<Vec<ParentReference>>, secret_name: &str) -> EdgionTls {
    EdgionTls {
        metadata: ObjectMeta {
            name: Some("test-tls".to_string()),
            namespace: Some("test-ns".to_string()),
            generation: Some(1),
            ..Default::default()
        },
        spec: edgion_resources::resources::edgion_tls::EdgionTlsSpec {
            current_status: None,
            parent_refs,
            hosts: vec!["example.com".to_string()],
            secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: secret_name.to_string(),
                namespace: Some("test-ns".to_string()),
            },
            client_auth: None,
            min_tls_version: None,
            ciphers: None,
            secret: None,
            resolved_listeners: None,
            resolved_listener_attachments: None,
            resolved_listener_attachment_index: Default::default(),
            resolved_log_labels: None,
        },
        status: None,
    }
}

#[tokio::test]
async fn parse_registers_parent_dependency_before_missing_gateway_lookup() {
    let ctx = make_ctx();
    let tls = make_tls(
        Some(vec![make_parent_ref("not-yet-published", "test-ns")]),
        "missing-secret",
    );
    let parsed = match EdgionTlsHandler::default().parse(tls, &ctx).await {
        ProcessResult::Continue(parsed) => parsed,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };
    assert!(parsed.spec.resolved_listener_attachments.is_none());
    assert_eq!(
        ctx.runtime()
            .gateway_route_index
            .get_routes_for_gateway("test-ns/not-yet-published"),
        vec![(ResourceKind::EdgionTls, "test-ns/test-tls".to_string())],
        "the dependency edge must exist even when the Gateway snapshot is absent"
    );
}

fn make_parent_ref(name: &str, ns: &str) -> ParentReference {
    ParentReference {
        group: None,
        kind: None,
        namespace: Some(ns.to_string()),
        name: name.to_string(),
        section_name: None,
        port: None,
    }
}

#[test]
fn test_update_status_with_validation_errors_sets_accepted_false() {
    let handler = EdgionTlsHandler::default();
    let ctx = make_ctx_with_gateways(&[("test-ns", "my-gateway")]);
    let parent_refs = vec![make_parent_ref("my-gateway", "test-ns")];
    let mut tls = make_tls(Some(parent_refs), "missing-secret");

    let errors = vec!["Secret 'test-ns/missing-secret' not found (may arrive later)".to_string()];
    handler.update_status(&mut tls, &ctx, &errors);

    let status = tls.status.as_ref().expect("status should be set");
    assert_eq!(status.parents.len(), 1);

    let conditions = &status.parents[0].conditions;
    let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
    assert_eq!(
        accepted.status, "False",
        "Accepted should be False when validation errors exist"
    );
    assert_eq!(accepted.reason, "Invalid");
    assert!(accepted.message.contains("Secret 'test-ns/missing-secret' not found"));
}

#[test]
fn test_update_status_no_errors_sets_accepted_true() {
    let handler = EdgionTlsHandler::default();
    let ctx = make_ctx_with_gateways(&[("test-ns", "my-gateway")]);
    let parent_refs = vec![make_parent_ref("my-gateway", "test-ns")];
    let mut tls = make_tls(Some(parent_refs), "existing-secret");

    handler.update_status(&mut tls, &ctx, &[]);

    let status = tls.status.as_ref().expect("status should be set");
    assert_eq!(status.parents.len(), 1);

    let conditions = &status.parents[0].conditions;
    let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
    assert_eq!(accepted.status, "True");

    let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
    assert_eq!(
        resolved.status, "False",
        "Secret not in global store so ResolvedRefs should be False"
    );
}

#[test]
fn test_update_status_no_parent_refs_still_initializes_status() {
    let handler = EdgionTlsHandler::default();
    let ctx = make_ctx();
    let mut tls = make_tls(None, "some-secret");

    let errors = vec!["EdgionTls has no parent_refs".to_string()];
    handler.update_status(&mut tls, &ctx, &errors);

    let status = tls.status.as_ref().expect("status should be initialized");
    assert!(
        status.parents.is_empty(),
        "no parent_refs means no per-parent status entries"
    );
}

#[test]
fn update_status_uses_fixed_empty_shape_for_invalid_dimensions() {
    let handler = EdgionTlsHandler::default();
    let ctx = make_ctx();
    let parents = (0..=edgion_resources::resources::edgion_tls::MAX_EDGION_TLS_PARENT_REFS)
        .map(|index| make_parent_ref(&format!("gateway-{index}"), "test-ns"))
        .collect();
    let mut tls = make_tls(Some(parents), "missing-secret");
    handler.update_status(&mut tls, &ctx, &["invalid dimensions".to_string()]);
    assert!(tls.status.unwrap().parents.is_empty());
}

#[test]
fn test_update_status_multiple_parents_all_get_validation_errors() {
    let handler = EdgionTlsHandler::default();
    let ctx = make_ctx_with_gateways(&[("test-ns", "gw-1"), ("test-ns", "gw-2")]);
    let parent_refs = vec![make_parent_ref("gw-1", "test-ns"), make_parent_ref("gw-2", "test-ns")];
    let mut tls = make_tls(Some(parent_refs), "bad-secret");

    let errors = vec!["Invalid TLS configuration".to_string()];
    handler.update_status(&mut tls, &ctx, &errors);

    let status = tls.status.as_ref().unwrap();
    assert_eq!(status.parents.len(), 2);

    for parent_status in &status.parents {
        let accepted = parent_status.conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(accepted.status, "False");
        assert_eq!(accepted.reason, "Invalid");
        assert!(accepted.message.contains("Invalid TLS configuration"));
    }
}

#[test]
fn test_update_status_updates_existing_parent_status() {
    let handler = EdgionTlsHandler::default();
    let ctx = make_ctx_with_gateways(&[("test-ns", "my-gw")]);
    let parent_refs = vec![make_parent_ref("my-gw", "test-ns")];
    let mut tls = make_tls(Some(parent_refs.clone()), "test-secret");

    handler.update_status(&mut tls, &ctx, &[]);
    let accepted = &tls.status.as_ref().unwrap().parents[0]
        .conditions
        .iter()
        .find(|c| c.type_ == "Accepted")
        .unwrap()
        .status;
    assert_eq!(accepted, "True");

    let errors = vec!["Secret expired".to_string()];
    handler.update_status(&mut tls, &ctx, &errors);

    let status = tls.status.as_ref().unwrap();
    assert_eq!(status.parents.len(), 1, "should update in-place, not duplicate");
    let accepted = status.parents[0]
        .conditions
        .iter()
        .find(|c| c.type_ == "Accepted")
        .unwrap();
    assert_eq!(accepted.status, "False");
    assert_eq!(accepted.reason, "Invalid");
}

#[test]
fn test_update_status_clears_parents_when_parent_refs_removed() {
    let handler = EdgionTlsHandler::default();
    let ctx = make_ctx();

    let parent_refs = vec![make_parent_ref("my-gw", "test-ns")];
    let mut tls = make_tls(Some(parent_refs), "test-secret");

    handler.update_status(&mut tls, &ctx, &[]);
    assert_eq!(tls.status.as_ref().unwrap().parents.len(), 1);

    tls.spec.parent_refs = None;
    handler.update_status(&mut tls, &ctx, &[]);
    assert!(
        tls.status.as_ref().unwrap().parents.is_empty(),
        "parents should be cleared when parent_refs is removed"
    );
}

#[test]
fn test_update_status_only_sets_accepted_and_resolved_refs() {
    let handler = EdgionTlsHandler::default();
    let ctx = make_ctx();
    let parent_refs = vec![make_parent_ref("my-gw", "test-ns")];
    let mut tls = make_tls(Some(parent_refs), "bad-secret");

    handler.update_status(&mut tls, &ctx, &[]);
    let conditions = &tls.status.as_ref().unwrap().parents[0].conditions;
    assert_eq!(conditions.len(), 2, "Only Accepted and ResolvedRefs should be set");
    assert!(conditions.iter().any(|c| c.type_ == "Accepted"));
    assert!(conditions.iter().any(|c| c.type_ == "ResolvedRefs"));
    assert!(
        !conditions.iter().any(|c| c.type_ == "Programmed"),
        "Programmed should not be set"
    );
    assert!(
        !conditions.iter().any(|c| c.type_ == "Ready"),
        "Ready should not be set"
    );

    let errors = vec!["Secret missing".to_string()];
    handler.update_status(&mut tls, &ctx, &errors);

    let conditions = &tls.status.as_ref().unwrap().parents[0].conditions;
    let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
    assert_eq!(accepted.status, "False");
    assert!(
        !conditions.iter().any(|c| c.type_ == "Programmed"),
        "Programmed should not be set"
    );
    assert!(
        !conditions.iter().any(|c| c.type_ == "Ready"),
        "Ready should not be set"
    );
}

/// Build a HandlerContext with a Secret (cert_pem + key_pem) populated in the secret store.
fn make_ctx_with_cert_secret(cert_pem: &str, key_pem: &str) -> HandlerContext {
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::constants::secret_keys::tls::{CERT, KEY};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::{BTreeMap, HashMap};

    let mut data = BTreeMap::new();
    data.insert(CERT.to_string(), ByteString(cert_pem.as_bytes().to_vec()));
    data.insert(KEY.to_string(), ByteString(key_pem.as_bytes().to_vec()));

    let secret = Secret {
        metadata: ObjectMeta {
            name: Some("test-secret".to_string()),
            namespace: Some("test-ns".to_string()),
            ..Default::default()
        },
        data: Some(data),
        ..Default::default()
    };

    let runtime = Arc::new(ControllerRuntime::default());
    let mut secrets = HashMap::new();
    secrets.insert("test-ns/test-secret".to_string(), secret);
    runtime.secret_store.replace_all(secrets);

    HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime)
}

#[test]
fn test_validate_detects_cert_key_mismatch() {
    use edgion_resources::constants::secret_keys::tls::{CERT, KEY};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use rcgen::{CertificateParams, KeyPair};
    use std::collections::BTreeMap;

    // Cert signed with key_a but embedding key_b — a key/cert mismatch.
    // Cert-content validation runs in update_status() via validate_cert(),
    // so the mismatch surfaces as ResolvedRefs/InvalidCertificate on the
    // per-parent status condition.
    let key_a = KeyPair::generate().unwrap();
    let cert_a = CertificateParams::new(vec!["example.com".to_string()])
        .unwrap()
        .self_signed(&key_a)
        .unwrap();
    let key_b = KeyPair::generate().unwrap();

    // Build the resolved Secret (cert from key_a, private key from key_b) and set it
    // directly on spec.secret to simulate the state after parse() has run.
    let mut data = BTreeMap::new();
    data.insert(CERT.to_string(), ByteString(cert_a.pem().as_bytes().to_vec()));
    data.insert(KEY.to_string(), ByteString(key_b.serialize_pem().as_bytes().to_vec()));
    let secret = Secret {
        metadata: ObjectMeta {
            name: Some("test-secret".to_string()),
            namespace: Some("test-ns".to_string()),
            ..Default::default()
        },
        data: Some(data),
        ..Default::default()
    };

    let handler = EdgionTlsHandler::default();
    let ctx = make_ctx_with_gateways(&[("test-ns", "my-gw")]);
    let mut tls = make_tls(Some(vec![make_parent_ref("my-gw", "test-ns")]), "test-secret");
    tls.spec.secret = Some(secret);

    handler.update_status(&mut tls, &ctx, &[]);

    let conditions = &tls.status.as_ref().unwrap().parents[0].conditions;
    let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
    assert_eq!(
        resolved.status, "False",
        "key/cert mismatch must set ResolvedRefs to False"
    );
    assert_eq!(
        resolved.reason, "InvalidCertificate",
        "key/cert mismatch must report reason=InvalidCertificate, not the old Accepted/Invalid warning"
    );
}

/// Build a HandlerContext whose secret store holds `<secret_ns>/<secret_name>`
/// (a `kubernetes.io/tls`-shaped Secret) and, when `grant_from_ns` is `Some`,
/// a ReferenceGrant in `secret_ns` allowing `edgion.io/EdgionTls` from that
/// namespace to reference `""/Secret`.
fn make_ctx_cross_ns(secret_ns: &str, secret_name: &str, grant_from_ns: Option<&str>) -> HandlerContext {
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::constants::secret_keys::tls::{CERT, KEY};
    use edgion_resources::resources::{ReferenceGrant, ReferenceGrantFrom, ReferenceGrantSpec, ReferenceGrantTo};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::{BTreeMap, HashMap};

    let mut data = BTreeMap::new();
    data.insert(CERT.to_string(), ByteString(b"cert-pem".to_vec()));
    data.insert(KEY.to_string(), ByteString(b"key-pem".to_vec()));
    let secret = Secret {
        metadata: ObjectMeta {
            name: Some(secret_name.to_string()),
            namespace: Some(secret_ns.to_string()),
            ..Default::default()
        },
        data: Some(data),
        ..Default::default()
    };

    let runtime = Arc::new(ControllerRuntime::default());
    let mut secrets = HashMap::new();
    secrets.insert(format!("{}/{}", secret_ns, secret_name), secret);
    runtime.secret_store.replace_all(secrets);

    if let Some(from_ns) = grant_from_ns {
        let grant = ReferenceGrant {
            metadata: ObjectMeta {
                name: Some("allow-edgiontls".to_string()),
                namespace: Some(secret_ns.to_string()),
                ..Default::default()
            },
            spec: ReferenceGrantSpec {
                from: vec![ReferenceGrantFrom {
                    group: "edgion.io".to_string(),
                    kind: "EdgionTls".to_string(),
                    namespace: from_ns.to_string(),
                }],
                to: vec![ReferenceGrantTo {
                    group: "".to_string(),
                    kind: "Secret".to_string(),
                    name: None,
                }],
            },
        };
        runtime
            .ref_grant_store
            .upsert(format!("{}/allow-edgiontls", secret_ns), Arc::new(grant));
    }

    HandlerContext::new(None, None, TriggerChain::default(), 3, 20, runtime)
}

/// EdgionTls in `owner_ns` whose `secret_ref` points at `secret_ns/secret_name`.
fn make_cross_ns_tls(owner_ns: &str, secret_ns: &str, secret_name: &str) -> EdgionTls {
    let mut tls = make_tls(None, secret_name);
    tls.metadata.namespace = Some(owner_ns.to_string());
    tls.spec.secret_ref.namespace = Some(secret_ns.to_string());
    tls
}

#[tokio::test]
async fn test_parse_denies_cross_ns_secret_without_grant() {
    // The Secret EXISTS in the store; only the missing ReferenceGrant must
    // keep it out of spec.secret. This is the core exfiltration guard.
    let ctx = make_ctx_cross_ns("victim-ns", "victim-secret", None);
    let handler = EdgionTlsHandler::default();
    let tls = make_cross_ns_tls("attacker-ns", "victim-ns", "victim-secret");

    let result = handler.parse(tls, &ctx).await;
    let tls = match result {
        ProcessResult::Continue(t) => t,
        ProcessResult::Skip { reason } => panic!("unexpected Skip: {reason}"),
    };
    assert!(
        tls.spec.secret.is_none(),
        "cross-namespace Secret without a ReferenceGrant must not be embedded"
    );
}

#[tokio::test]
async fn test_parse_allows_cross_ns_secret_with_grant() {
    let ctx = make_ctx_cross_ns("victim-ns", "victim-secret", Some("attacker-ns"));
    let handler = EdgionTlsHandler::default();
    let tls = make_cross_ns_tls("attacker-ns", "victim-ns", "victim-secret");

    let result = handler.parse(tls, &ctx).await;
    let tls = match result {
        ProcessResult::Continue(t) => t,
        ProcessResult::Skip { reason } => panic!("unexpected Skip: {reason}"),
    };
    assert!(
        tls.spec.secret.is_some(),
        "cross-namespace Secret WITH a matching ReferenceGrant should resolve"
    );
}

#[tokio::test]
async fn test_parse_resolves_same_ns_secret() {
    // Same-namespace ref is never a cross-ns reference, so no grant is needed.
    let ctx = make_ctx_cross_ns("test-ns", "test-secret", None);
    let handler = EdgionTlsHandler::default();
    let tls = make_cross_ns_tls("test-ns", "test-ns", "test-secret");

    let result = handler.parse(tls, &ctx).await;
    let tls = match result {
        ProcessResult::Continue(t) => t,
        ProcessResult::Skip { reason } => panic!("unexpected Skip: {reason}"),
    };
    assert!(
        tls.spec.secret.is_some(),
        "same-namespace Secret should resolve without a grant"
    );
}

#[tokio::test]
async fn invalid_group_or_kind_never_registers_grants_or_reads_secret() {
    let ctx = make_ctx_cross_ns("test-ns", "test-secret", None);
    let handler = EdgionTlsHandler::default();
    let mut tls = make_tls(Some(vec![make_parent_ref("my-gw", "test-ns")]), "test-secret");
    tls.spec.secret_ref.group = Some("core".to_string());
    tls.spec.secret_ref.kind = Some("ConfigMap".to_string());

    let warnings = handler.validate(&tls, &ctx);
    assert!(warnings.iter().any(|warning| warning.contains("core Secret kind")));

    let parsed = match handler.parse(tls, &ctx).await {
        ProcessResult::Continue(tls) => tls,
        ProcessResult::Skip { reason } => panic!("unexpected Skip: {reason}"),
    };
    assert!(parsed.spec.secret.is_none());
    assert!(ctx.secret_ref_manager().get_refs("test-ns/test-secret").is_empty());
    assert!(ctx.runtime().cross_ns_ref_mgr.get_refs("test-ns").is_empty());

    let mut status_tls = parsed;
    handler.update_status(&mut status_tls, &ctx, &warnings);
    let conditions = &status_tls.status.as_ref().unwrap().parents[0].conditions;
    let resolved = conditions
        .iter()
        .find(|condition| condition.type_ == "ResolvedRefs")
        .unwrap();
    assert_eq!(resolved.status, "False");
    assert_eq!(resolved.reason, "InvalidKind");

    let mut tls = make_tls(Some(vec![make_parent_ref("my-gw", "test-ns")]), "test-secret");
    tls.spec.client_auth = Some(ClientAuthConfig {
        mode: ClientAuthMode::Mutual,
        ca_secret_ref: Some(SecretObjectReference {
            group: Some("core".to_string()),
            kind: Some("ConfigMap".to_string()),
            namespace: Some("test-ns".to_string()),
            name: "test-secret".to_string(),
        }),
        verify_depth: 1,
        ca_secret: None,
        allowed_sans: None,
        allowed_cns: None,
    });
    let warnings = handler.validate(&tls, &ctx);
    let mut parsed = match handler.parse(tls, &ctx).await {
        ProcessResult::Continue(tls) => tls,
        ProcessResult::Skip { reason } => panic!("unexpected Skip: {reason}"),
    };
    assert!(parsed.spec.client_auth.as_ref().unwrap().ca_secret.is_none());
    handler.update_status(&mut parsed, &ctx, &warnings);
    assert!(parsed.status.as_ref().unwrap().parents[0]
        .conditions
        .iter()
        .any(|condition| condition.type_ == "ResolvedRefs"
            && condition.status == "False"
            && condition.reason == "InvalidKind"));
}

#[tokio::test]
async fn test_parse_registers_cross_ns_ref_for_requeue_on_grant_change() {
    // A denied cross-ns ref must still be tracked in cross_ns_ref_mgr so that a
    // later ReferenceGrant addition in the target namespace requeues this EdgionTls
    // (otherwise it would stay stuck in RefNotPermitted forever).
    let ctx = make_ctx_cross_ns("victim-ns", "victim-secret", None);
    let handler = EdgionTlsHandler::default();
    let tls = make_cross_ns_tls("attacker-ns", "victim-ns", "victim-secret");
    let expected_ref = ResourceRef::new(
        ResourceKind::EdgionTls,
        Some("attacker-ns".to_string()),
        "test-tls".to_string(),
    );

    let _ = handler.parse(tls, &ctx).await;

    let tracked = ctx.runtime().cross_ns_ref_mgr.get_refs("victim-ns");
    assert!(
        tracked.contains(&expected_ref),
        "cross-ns secret ref must be tracked under the target namespace, got: {:?}",
        tracked
    );
}

/// Verify that an expired certificate surfaces as ResolvedRefs=False/InvalidCertificate
/// and that the condition message contains NO wall-clock timestamp — only the cert's own
/// validity window (deterministic per-cert, no `current` field from status_message()).
#[test]
fn expired_cert_sets_resolved_refs_false_without_clock() {
    use edgion_resources::constants::secret_keys::tls::{CERT, KEY};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use rcgen::{CertificateParams, KeyPair};
    use std::collections::BTreeMap;

    // Mint a cert whose validity window is entirely in the past (~2 years ago).
    // Using offsets > 730 days ensures the cert dates fall in a different calendar
    // year than today, so no part of the message overlaps with the current year.
    let now = ::time::OffsetDateTime::now_utc();
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec!["example.com".to_string()]).unwrap();
    params.not_before = now - ::time::Duration::days(800);
    params.not_after = now - ::time::Duration::days(730);
    let cert = params.self_signed(&key).unwrap();

    let mut data = BTreeMap::new();
    data.insert(CERT.to_string(), ByteString(cert.pem().as_bytes().to_vec()));
    data.insert(KEY.to_string(), ByteString(key.serialize_pem().as_bytes().to_vec()));
    let secret = Secret {
        metadata: ObjectMeta {
            name: Some("test-secret".to_string()),
            namespace: Some("test-ns".to_string()),
            ..Default::default()
        },
        data: Some(data),
        ..Default::default()
    };

    let handler = EdgionTlsHandler::default();
    let ctx = make_ctx_with_gateways(&[("test-ns", "my-gateway")]);
    let mut tls = make_tls(Some(vec![make_parent_ref("my-gateway", "test-ns")]), "test-secret");
    tls.spec.secret = Some(secret);

    handler.update_status(&mut tls, &ctx, &[]);

    let conditions = &tls.status.as_ref().unwrap().parents[0].conditions;
    let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();

    assert_eq!(resolved.status, "False", "expired cert must set ResolvedRefs=False");
    assert_eq!(
        resolved.reason, "InvalidCertificate",
        "expired cert reason must be InvalidCertificate"
    );

    // status_message() must not include the current wall-clock time (the `current` field
    // of CertExpired is intentionally omitted — only not_before/not_after are included).
    let current_year = now.year().to_string();
    assert!(
        !resolved.message.contains(&current_year),
        "condition message must not contain the current calendar year (wall-clock leak): {}",
        resolved.message
    );
    assert!(
        !resolved.message.to_lowercase().contains("current"),
        "condition message must not contain the word 'current': {}",
        resolved.message
    );
}

#[test]
fn test_update_status_reports_ref_not_permitted_for_cross_ns_without_grant() {
    let ctx = make_ctx_cross_ns("victim-ns", "victim-secret", None);
    let handler = EdgionTlsHandler::default();
    let mut tls = make_cross_ns_tls("attacker-ns", "victim-ns", "victim-secret");
    tls.spec.parent_refs = Some(vec![make_parent_ref("my-gw", "attacker-ns")]);
    // spec.secret stays None (the denied/unresolved state parse() leaves behind).

    handler.update_status(&mut tls, &ctx, &[]);

    let conditions = &tls.status.as_ref().unwrap().parents[0].conditions;
    let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
    assert_eq!(resolved.status, "False");
    assert_eq!(
        resolved.reason, "RefNotPermitted",
        "cross-ns denial should surface RefNotPermitted, not a perpetual SecretNotFound"
    );
}

#[test]
fn test_validate_passes_matching_cert_key() {
    use edgion_resources::constants::secret_keys::tls::{CERT, KEY};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use rcgen::{CertificateParams, KeyPair};
    use std::collections::BTreeMap;

    let key = KeyPair::generate().unwrap();
    let cert = CertificateParams::new(vec!["example.com".to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();

    // validate() no longer checks cert content; it only warns on missing parent_refs
    // and missing secrets. A matching cert+key must not produce any spurious warning
    // in validate(), and the positive path in update_status must yield ResolvedRefs=True.
    let ctx = make_ctx_with_cert_secret(&cert.pem(), &key.serialize_pem());
    let handler = EdgionTlsHandler::default();
    let tls = make_tls(Some(vec![make_parent_ref("my-gw", "test-ns")]), "test-secret");

    let warnings = handler.validate(&tls, &ctx);
    assert!(
        !warnings.iter().any(|w| w.contains("does not match")),
        "matching cert/key should not produce mismatch warning, got: {:?}",
        warnings
    );

    // Positive path: update_status with the resolved secret must yield ResolvedRefs=True.
    let mut data = BTreeMap::new();
    data.insert(CERT.to_string(), ByteString(cert.pem().as_bytes().to_vec()));
    data.insert(KEY.to_string(), ByteString(key.serialize_pem().as_bytes().to_vec()));
    let secret = Secret {
        metadata: ObjectMeta {
            name: Some("test-secret".to_string()),
            namespace: Some("test-ns".to_string()),
            ..Default::default()
        },
        data: Some(data),
        ..Default::default()
    };
    let ctx_gw = make_ctx_with_gateways(&[("test-ns", "my-gw")]);
    let mut tls2 = make_tls(Some(vec![make_parent_ref("my-gw", "test-ns")]), "test-secret");
    tls2.spec.secret = Some(secret);

    handler.update_status(&mut tls2, &ctx_gw, &[]);

    let conditions = &tls2.status.as_ref().unwrap().parents[0].conditions;
    let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
    assert_eq!(
        resolved.status, "True",
        "matching cert/key must set ResolvedRefs=True, got: {:?}",
        resolved
    );
}

#[tokio::test]
async fn actual_fields_scrub_injection_independently_and_status_is_readonly() {
    use crate::conf_mgr::sync_runtime::resource_processor::credential::pipeline::observe_bound_reads;
    let ctx = make_ctx_cross_ns("test-ns", "test-secret", None);
    let handler = EdgionTlsHandler::default();
    let mut tls = make_tls(Some(vec![make_parent_ref("my-gw", "test-ns")]), "test-secret");
    tls.spec.secret = Some(Default::default());
    tls.spec.client_auth = Some(
        serde_json::from_value(serde_json::json!({"mode":"Mutual","caSecretRef":{"name":"ca","namespace":"target"}}))
            .unwrap(),
    );
    tls.spec.client_auth.as_mut().unwrap().ca_secret = Some(Default::default());
    let ProcessResult::Continue(mut parsed) = handler.parse(tls, &ctx).await else {
        panic!("parse skipped")
    };
    assert!(parsed.spec.secret.is_some());
    assert!(parsed.spec.client_auth.as_ref().unwrap().ca_secret.is_none());
    let owner = ResourceRef::new(ResourceKind::EdgionTls, Some("test-ns".into()), "test-tls".into());
    let edges = ctx.secret_ref_manager().get_dependencies(&owner.key());
    assert!(edges.contains(&"test-ns/test-secret".into()));
    assert!(edges.contains(&"target/ca".into()));
    assert!(ctx.runtime().cross_ns_ref_mgr.get_refs("target").contains(&owner));
    let (_, reads) = observe_bound_reads(|| handler.update_status(&mut parsed, &ctx, &[]));
    assert!(!reads.contains(&"secret"));
    assert_eq!(ctx.secret_ref_manager().get_dependencies(&owner.key()), edges);
    assert!(parsed.status.as_ref().unwrap().parents[0]
        .conditions
        .iter()
        .any(|c| c.type_ == "ResolvedRefs" && c.reason == "RefNotPermitted"));
}
