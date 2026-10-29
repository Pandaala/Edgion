use super::*;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::resources::common::CustomCACertificateRef;
use edgion_resources::resources::common::{OutboundTlsConfig, OutboundTlsValidation};
use edgion_resources::resources::edgion_gateway_config::{EdgionGatewayConfigSpec, MAX_RETRY_ATTEMPTS};
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

fn make_ctx_with_secrets(secrets: HashMap<String, Secret>) -> HandlerContext {
    let runtime = Arc::new(ControllerRuntime::default());
    runtime.secret_store.replace_all(secrets);
    HandlerContext::new(None, None, Default::default(), 5, 20, runtime)
}

#[tokio::test]
async fn administrative_tls_keeps_order_provenance_and_independent_axes_without_grants() {
    use edgion_resources::constants::annotations::edgion::{CONTROLLER_CA_SOURCE, CONTROLLER_CA_SOURCE_CONFIG_MAP};
    let mut secret = Secret {
        metadata: kube::api::ObjectMeta {
            name: Some("shared".into()),
            namespace: Some("certs".into()),
            annotations: Some(BTreeMap::from([(
                CONTROLLER_CA_SOURCE.into(),
                CONTROLLER_CA_SOURCE_CONFIG_MAP.into(),
            )])),
            ..Default::default()
        },
        data: Some(BTreeMap::from([("ca.crt".into(), ByteString(b"secret-ca".to_vec()))])),
        ..Default::default()
    };
    let ctx = make_ctx_with_secrets(HashMap::from([("certs/shared".into(), secret.clone())]));
    let configmap: k8s_openapi::api::core::v1::ConfigMap = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"shared","namespace":"certs","uid":"actual-cm"},"data":{"ca.crt":"configmap-ca"}}))
    .unwrap();
    ctx.runtime()
        .configmap_store
        .replace_all(HashMap::from([("certs/shared".into(), configmap)]));
    let owner = ResourceRef::new(ResourceKind::EdgionGatewayConfig, None, "global".into());
    let config: EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"global"},"spec":{"outboundTls":{
            "validation":{"caCertificateRefs":[
                {"kind":"Secret","name":"shared","namespace":"certs"},
                {"kind":"Secret","name":"bad"},
                {"group":"core","kind":"ConfigMap","name":"shared","namespace":"certs"},
                {"kind":"Secret","name":"shared","namespace":"certs"},
                {"kind":"Secret","name":"missing","namespace":"certs"}]},
            "clientCertificateRef":{"name":"shared","namespace":"certs"}}}}))
    .unwrap();
    let prepared = PreparedEgcTlsReferences::from_config(&config);
    let binding = prepared.bind(&config).unwrap();
    let references = binding.references().unwrap();
    let selection = references.client_certificate().unwrap().unwrap();
    // An ordinary namespaced acquisition cannot turn a cluster token into
    // administrative authority; only the private EGC selection path can read.
    assert!(matches!(
        crate::conf_mgr::sync_runtime::resource_processor::credential::fetch_bound_secret(
            &ctx,
            &owner,
            *selection.reference()
        ),
        crate::conf_mgr::sync_runtime::resource_processor::credential::BoundFetchOutcome::Invalid(_)
    ));
    assert!(ctx.secret_ref_manager().get_dependencies(&owner.key()).is_empty());
    let parsed = parse_client_identity(config, &ctx).await;
    let field = parsed.spec.outbound_tls.as_ref().unwrap();
    let ca = field.resolved_ca_certificates.as_ref().unwrap();
    assert_eq!(ca.len(), 3);
    assert_eq!(
        ca[0]
            .metadata
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.get(CONTROLLER_CA_SOURCE)),
        None
    );
    assert_eq!(
        ca[1].metadata.annotations.as_ref().unwrap()[CONTROLLER_CA_SOURCE],
        CONTROLLER_CA_SOURCE_CONFIG_MAP
    );
    assert_eq!(ca[1].metadata.uid.as_deref(), Some("actual-cm"));
    assert_eq!(ca[0].data, ca[2].data);
    assert_eq!(
        field
            .resolved_client_certificate
            .as_ref()
            .unwrap()
            .metadata
            .name
            .as_deref(),
        Some("shared")
    );
    assert!(field
        .resolved_client_certificate
        .as_ref()
        .unwrap()
        .metadata
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get(CONTROLLER_CA_SOURCE))
        .is_none());
    assert!(ctx.secret_ref_manager().get_refs("certs/missing").contains(&owner));
    assert!(ctx.secret_ref_manager().get_refs("bad").is_empty());
    assert!(ctx.runtime().cross_ns_ref_mgr.get_refs("certs").is_empty());
    secret.metadata.annotations = None;
    ctx.runtime()
        .secret_store
        .replace_all(HashMap::from([("certs/shared".into(), secret)]));
    let mut changed = parsed;
    changed
        .spec
        .outbound_tls
        .as_mut()
        .unwrap()
        .client_certificate_ref
        .as_mut()
        .unwrap()
        .name = "missing-client".into();
    let changed = parse_client_identity(changed, &ctx).await;
    assert!(changed
        .spec
        .outbound_tls
        .as_ref()
        .unwrap()
        .resolved_client_certificate
        .is_none());
    assert_eq!(
        changed
            .spec
            .outbound_tls
            .as_ref()
            .unwrap()
            .resolved_ca_certificates
            .as_ref()
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn malformed_cluster_namespace_clears_prior_canonical_owner_dependencies_and_materials() {
    let ctx = make_ctx_with_secrets(HashMap::new());
    let owner = ResourceRef::new(ResourceKind::EdgionGatewayConfig, None, "global-client".into());
    let mut config = parse_client_identity(client_identity_config(Some("certs")), &ctx).await;
    assert!(ctx.secret_ref_manager().get_refs("certs/client").contains(&owner));
    ctx.runtime().cross_ns_ref_mgr.add_ref("certs".into(), owner.clone());
    config.metadata.namespace = Some("fake".into());
    config.spec.outbound_tls.as_mut().unwrap().resolved_client_certificate = Some(Secret::default());
    let mut parsed = parse_client_identity(config, &ctx).await;
    assert!(ctx.secret_ref_manager().get_dependencies(&owner.key()).is_empty());
    assert!(ctx.runtime().cross_ns_ref_mgr.get_refs("certs").is_empty());
    assert!(parsed
        .spec
        .outbound_tls
        .as_ref()
        .unwrap()
        .resolved_client_certificate
        .is_none());
    assert!(!EdgionGatewayConfigHandler.preparse(&mut parsed, &ctx).is_empty());
    parsed.spec.outbound_tls = None;
    assert!(!EdgionGatewayConfigHandler.preparse(&mut parsed, &ctx).is_empty());
}

#[test]
fn intrinsic_cluster_tls_acceptance_is_independent_of_material_visibility_and_has_no_io() {
    let visible = make_ctx_with_secrets(HashMap::from([("certs/client".into(), Secret::default())]));
    let hidden = make_ctx_with_secrets(HashMap::new());
    for invalid in [false, true] {
        let mut original = client_identity_config(Some("certs"));
        if invalid {
            original
                .spec
                .outbound_tls
                .as_mut()
                .unwrap()
                .client_certificate_ref
                .as_mut()
                .unwrap()
                .namespace = None;
        }
        for ctx in [&visible, &hidden] {
            let mut config = original.clone();
            let errors = EdgionGatewayConfigHandler.preparse(&mut config, ctx);
            assert_eq!(errors.is_empty(), !invalid);
            EdgionGatewayConfigHandler.update_status(&mut config, ctx, &errors);
            let condition = config
                .status
                .as_ref()
                .unwrap()
                .conditions
                .iter()
                .find(|condition| condition.type_ == "Accepted")
                .unwrap();
            assert_eq!(condition.status, if invalid { "False" } else { "True" });
            assert!(config
                .status
                .as_ref()
                .unwrap()
                .conditions
                .iter()
                .all(|condition| condition.type_ != "ResolvedRefs"));
            let owner = ResourceRef::new(ResourceKind::EdgionGatewayConfig, None, "global-client".into());
            assert!(ctx.secret_ref_manager().get_dependencies(&owner.key()).is_empty());
        }
    }
}

#[test]
fn preparse_rejects_retry_attempts_above_route_domain() {
    let ctx = make_ctx_with_secrets(HashMap::new());
    let handler = EdgionGatewayConfigHandler::new();
    let mut egc = EdgionGatewayConfig::new("global", EdgionGatewayConfigSpec::default());
    assert!(handler.preparse(&mut egc, &ctx).is_empty());

    egc.spec.retry.attempts = MAX_RETRY_ATTEMPTS + 1;
    let errors = handler.preparse(&mut egc, &ctx);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("retry.attempts"));
}

#[tokio::test]
async fn global_plugin_refs_replace_dependencies_and_clear_on_delete() {
    use edgion_resources::resources::edgion_gateway_config::PluginReference;
    let ctx = make_ctx_with_secrets(HashMap::new());
    let handler = EdgionGatewayConfigHandler::new();
    let mut egc = EdgionGatewayConfig::new("global", EdgionGatewayConfigSpec::default());
    for name in ["first", "second"] {
        egc.spec.global_plugins_ref = Some(vec![PluginReference {
            name: name.to_string(),
            namespace: None,
        }]);
        let _ = handler.parse(egc.clone(), &ctx).await;
    }
    assert!(ctx.runtime().plugin_ref_mgr.get_refs("default/first").is_empty());
    assert_eq!(ctx.runtime().plugin_ref_mgr.get_refs("default/second").len(), 1);
    handler.on_delete(&egc, &ctx).await;
    assert!(ctx.runtime().plugin_ref_mgr.get_refs("default/second").is_empty());
}

// Raw-generation early-plugin validation regression coverage moved to
// gateway_class's global_request_id_validation_observes_replaced_raw_generation.

/// A cluster-scoped EdgionGatewayConfig whose `outboundTls.validation.caCertificateRefs` points
/// at a Secret present in the store must have `resolved_ca_secret` filled after
/// `parse`, and register the reference with SecretRefManager.
#[tokio::test]
async fn parse_resolves_global_outbound_ca_secret() {
    let mut data = BTreeMap::new();
    data.insert(
        "ca.crt".to_string(),
        ByteString(b"-----BEGIN CERTIFICATE-----\nMIIBglobal\n-----END CERTIFICATE-----\n".to_vec()),
    );
    let ca_secret = Secret {
        metadata: kube::api::ObjectMeta {
            name: Some("global-ca".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        data: Some(data),
        type_: Some("kubernetes.io/tls".to_string()),
        ..Default::default()
    };
    let mut secrets = HashMap::new();
    // The cluster owner selects this explicit target namespace administratively.
    secrets.insert("default/global-ca".to_string(), ca_secret);

    let ctx = make_ctx_with_secrets(secrets);

    let spec = EdgionGatewayConfigSpec {
        current_status: None,
        outbound_tls: Some(OutboundTlsConfig {
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![CustomCACertificateRef {
                    kind: "Secret".to_string(),
                    name: "global-ca".to_string(),
                    namespace: Some("default".to_string()),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    let egc = EdgionGatewayConfig::new("cluster-egc", spec);

    let handler = EdgionGatewayConfigHandler::new();
    let parsed = match handler.parse(egc, &ctx).await {
        ProcessResult::Continue(v) => v,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };

    assert!(
        parsed
            .spec
            .outbound_tls
            .unwrap()
            .resolved_ca_certificates
            .as_ref()
            .is_some_and(|v| !v.is_empty()),
        "resolved_ca_certificates must be filled from the resolved Secret"
    );
    assert_eq!(ctx.secret_ref_manager().get_refs("default/global-ca").len(), 1);
}

/// After `parse` registers a secret ref for the outbound CA, `on_delete` must
/// clear it so a later Secret change does not spuriously requeue a deleted EGC.
#[tokio::test]
async fn on_delete_clears_secret_refs() {
    let mut data = BTreeMap::new();
    data.insert(
        "ca.crt".to_string(),
        ByteString(b"-----BEGIN CERTIFICATE-----\nMIIBglobal\n-----END CERTIFICATE-----\n".to_vec()),
    );
    let ca_secret = Secret {
        metadata: kube::api::ObjectMeta {
            name: Some("global-ca".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        data: Some(data),
        type_: Some("kubernetes.io/tls".to_string()),
        ..Default::default()
    };
    let mut secrets = HashMap::new();
    secrets.insert("default/global-ca".to_string(), ca_secret);

    let ctx = make_ctx_with_secrets(secrets);

    let spec = EdgionGatewayConfigSpec {
        current_status: None,
        outbound_tls: Some(OutboundTlsConfig {
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![CustomCACertificateRef {
                    kind: "Secret".to_string(),
                    name: "global-ca".to_string(),
                    namespace: Some("default".to_string()),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    let egc = EdgionGatewayConfig::new("cluster-egc", spec);

    let handler = EdgionGatewayConfigHandler::new();
    // parse registers the ref
    let parsed = match handler.parse(egc, &ctx).await {
        ProcessResult::Continue(v) => v,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };
    assert_eq!(
        ctx.secret_ref_manager().get_refs("default/global-ca").len(),
        1,
        "ref must be registered after parse"
    );

    // on_delete must clear it
    handler.on_delete(&parsed, &ctx).await;
    assert_eq!(
        ctx.secret_ref_manager().get_refs("default/global-ca").len(),
        0,
        "ref must be cleared after on_delete"
    );
}

async fn assert_cross_namespace_ca_refs_replace_dependencies_and_clear_on_delete(kind: &str) {
    let ctx = make_ctx_with_secrets(HashMap::new());
    let handler = EdgionGatewayConfigHandler::new();
    let spec = EdgionGatewayConfigSpec {
        current_status: None,
        outbound_tls: Some(OutboundTlsConfig {
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![CustomCACertificateRef {
                    kind: kind.to_string(),
                    name: "global-ca".to_string(),
                    namespace: Some("first-target".to_string()),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut egc = EdgionGatewayConfig::new("cluster-egc", spec);
    let expected_ref = ResourceRef::new(ResourceKind::EdgionGatewayConfig, None, "cluster-egc".to_string());
    let material_key = |namespace: &str| {
        if kind == "ConfigMap" {
            format!("ConfigMap/{namespace}/global-ca")
        } else {
            format!("{namespace}/global-ca")
        }
    };

    let _ = handler.parse(egc.clone(), &ctx).await;
    assert!(!ctx
        .runtime()
        .cross_ns_ref_mgr
        .get_refs("first-target")
        .contains(&expected_ref));
    assert!(ctx
        .secret_ref_manager()
        .get_refs(&material_key("first-target"))
        .contains(&expected_ref));

    egc.spec
        .outbound_tls
        .as_mut()
        .unwrap()
        .validation
        .as_mut()
        .unwrap()
        .ca_certificate_refs
        .as_mut()
        .unwrap()[0]
        .namespace = Some("second-target".to_string());
    let parsed = match handler.parse(egc, &ctx).await {
        ProcessResult::Continue(value) => value,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };
    assert!(ctx.runtime().cross_ns_ref_mgr.get_refs("first-target").is_empty());
    assert!(ctx
        .secret_ref_manager()
        .get_refs(&material_key("first-target"))
        .is_empty());
    assert!(!ctx
        .runtime()
        .cross_ns_ref_mgr
        .get_refs("second-target")
        .contains(&expected_ref));
    assert!(ctx
        .secret_ref_manager()
        .get_refs(&material_key("second-target"))
        .contains(&expected_ref));

    handler.on_delete(&parsed, &ctx).await;
    assert!(ctx.runtime().cross_ns_ref_mgr.get_refs("second-target").is_empty());
    assert!(ctx
        .secret_ref_manager()
        .get_refs(&material_key("second-target"))
        .is_empty());
}

#[tokio::test]
async fn cross_namespace_secret_ca_refs_replace_dependencies_and_clear_on_delete() {
    assert_cross_namespace_ca_refs_replace_dependencies_and_clear_on_delete("Secret").await;
}

#[tokio::test]
async fn cross_namespace_configmap_ca_refs_replace_dependencies_and_clear_on_delete() {
    assert_cross_namespace_ca_refs_replace_dependencies_and_clear_on_delete("ConfigMap").await;
}

// ── Phase 1 SAN guard tests ───────────────────────────────────────────────

/// EGC with `outboundTls.validation.subjectAltNames` must produce an
/// Accepted=False preparse error in Phase 1.
#[tokio::test]
async fn egc_with_san_rejected_phase1() {
    use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};

    let ctx = make_ctx_with_secrets(HashMap::new());

    let spec = EdgionGatewayConfigSpec {
        current_status: None,
        outbound_tls: Some(OutboundTlsConfig {
            validation: Some(OutboundTlsValidation {
                subject_alt_names: Some(vec![SubjectAltName {
                    san_type: SubjectAltNameType::Hostname,
                    hostname: Some("upstream.example.com".to_string()),
                    uri: None,
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut egc = EdgionGatewayConfig::new("cluster-egc-san", spec);

    let handler = EdgionGatewayConfigHandler::new();
    let errors = handler.preparse(&mut egc, &ctx);

    assert!(
        !errors.is_empty(),
        "expected preparse error for EGC subjectAltNames in Phase 1"
    );
    assert!(
        errors.iter().any(|e| e.contains("EdgionGatewayConfig")),
        "error must identify the resource type; got: {:?}",
        errors
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("SAN pinning") || e.contains("subjectAltNames")),
        "error must mention SAN pinning; got: {:?}",
        errors
    );
}

/// A malformed `spec.requestBody.maxBodySize` must surface as a preparse error
/// (Accepted=False) via the handler.
#[tokio::test]
async fn egc_bad_max_body_size_rejected() {
    let ctx = make_ctx_with_secrets(HashMap::new());
    let spec = EdgionGatewayConfigSpec {
        current_status: None,
        request_body: edgion_resources::resources::edgion_gateway_config::RequestBodyConfig {
            max_body_size: "garbage".to_string(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut egc = EdgionGatewayConfig::new("cluster-egc-mbs", spec);

    let errors = EdgionGatewayConfigHandler::new().preparse(&mut egc, &ctx);
    assert!(
        errors.iter().any(|e| e.contains("maxBodySize")),
        "expected a maxBodySize preparse error; got: {errors:?}"
    );
}

#[test]
fn egc_plugin_policy_invalid_names_surface_in_preparse() {
    let ctx = make_ctx_with_secrets(HashMap::new());
    let spec: EdgionGatewayConfigSpec =
        serde_yaml::from_str("pluginPolicy:\n  deny:\n    - name: http/Unknown").unwrap();
    let mut egc = EdgionGatewayConfig::new("cluster-egc-policy", spec);
    let errors = EdgionGatewayConfigHandler::new().preparse(&mut egc, &ctx);
    assert!(errors
        .iter()
        .any(|error| error.contains("pluginPolicy: unknown plugin")));
    assert!(
        egc.spec.plugin_policy.is_some(),
        "advisory validation must retain policy input"
    );

    egc.spec.plugin_policy = Some(serde_yaml::from_str("allow: []").unwrap());
    assert!(EdgionGatewayConfigHandler::new().preparse(&mut egc, &ctx).is_empty());
}

#[test]
fn egc_reserved_remote_ip_header_is_rejected_at_ingest() {
    let ctx = make_ctx_with_secrets(HashMap::new());
    let mut spec = EdgionGatewayConfigSpec::default();
    spec.forwarded_headers.remote_ip_header = "X-Forwarded-For".to_string();
    let mut egc = EdgionGatewayConfig::new("cluster-egc-forwarded", spec);

    let errors = EdgionGatewayConfigHandler::new().preparse(&mut egc, &ctx);
    assert!(
        errors.iter().any(|e| e.contains("remoteIpHeader")),
        "expected remoteIpHeader validation error; got: {errors:?}"
    );
}

#[test]
fn egc_invalid_real_ip_header_is_rejected_at_ingest() {
    use edgion_resources::resources::edgion_plugins::RealIpConfig;

    let ctx = make_ctx_with_secrets(HashMap::new());
    let spec = EdgionGatewayConfigSpec {
        current_status: None,
        real_ip: Some(RealIpConfig {
            trusted: serde_json::from_value::<RealIpConfig>(
                serde_json::json!({"trustedIpRefs": [{"name": "internal", "namespace": "default"}]}),
            )
            .unwrap()
            .trusted,
            real_ip_header: "X Forwarded For".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut egc = EdgionGatewayConfig::new("cluster-egc-real-ip", spec);

    let errors = EdgionGatewayConfigHandler::new().preparse(&mut egc, &ctx);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("realIp") && e.contains("valid HTTP header name")),
        "expected realIp validation error; got: {errors:?}"
    );
}

/// EGC with `outboundTls.validation.hostname` must also be rejected in Phase 1.
#[tokio::test]
async fn egc_with_hostname_rejected_phase1() {
    let ctx = make_ctx_with_secrets(HashMap::new());

    let spec = EdgionGatewayConfigSpec {
        current_status: None,
        outbound_tls: Some(OutboundTlsConfig {
            validation: Some(OutboundTlsValidation {
                hostname: Some("upstream.example.com".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut egc = EdgionGatewayConfig::new("cluster-egc-host", spec);

    let handler = EdgionGatewayConfigHandler::new();
    let errors = handler.preparse(&mut egc, &ctx);

    assert!(
        !errors.is_empty(),
        "expected preparse error for EGC hostname in Phase 1"
    );
    assert!(
        errors.iter().any(|e| e.contains("EdgionGatewayConfig")),
        "error must identify the resource type; got: {:?}",
        errors
    );
}

/// EGC with only CA refs (no SAN fields) must NOT be rejected by the SAN guard.
#[tokio::test]
async fn egc_ca_only_no_san_accepted() {
    let ctx = make_ctx_with_secrets(HashMap::new());

    let spec = EdgionGatewayConfigSpec {
        current_status: None,
        outbound_tls: Some(OutboundTlsConfig {
            validation: Some(OutboundTlsValidation {
                ca_certificate_refs: Some(vec![CustomCACertificateRef {
                    kind: "Secret".to_string(),
                    name: "global-ca".to_string(),
                    namespace: Some("default".to_string()),
                    ..Default::default()
                }]),
                subject_alt_names: None,
                hostname: None,
                well_known_ca_certificates: None,
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut egc = EdgionGatewayConfig::new("cluster-egc-ca", spec);

    let handler = EdgionGatewayConfigHandler::new();
    let errors = handler.preparse(&mut egc, &ctx);

    let san_errors: Vec<_> = errors
        .iter()
        .filter(|e| e.contains("SAN pinning") || e.contains("subjectAltNames"))
        .collect();
    assert!(
        san_errors.is_empty(),
        "CA-only EGC must not be rejected by the SAN guard; got: {:?}",
        errors
    );
}

fn client_identity_config(namespace: Option<&str>) -> EdgionGatewayConfig {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "edgion.io/v1alpha1", "kind": "EdgionGatewayConfig",
        "metadata": {"name": "global-client"},
        "spec": {"outboundTls": {"clientCertificateRef": {
            "name": "client", "namespace": namespace
        }}}
    }))
    .unwrap()
}

async fn parse_client_identity(egc: EdgionGatewayConfig, ctx: &HandlerContext) -> EdgionGatewayConfig {
    match EdgionGatewayConfigHandler.parse(egc, ctx).await {
        ProcessResult::Continue(value) => value,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    }
}

#[tokio::test]
async fn global_client_identity_tracks_material_grants_and_removal() {
    let ctx = make_ctx_with_secrets(HashMap::new());
    let owner = ResourceRef::new(ResourceKind::EdgionGatewayConfig, None, "global-client".into());
    let mut egc = client_identity_config(Some("certs"));
    egc.spec.outbound_tls.as_mut().unwrap().resolved_client_certificate = Some(Secret::default());
    egc = parse_client_identity(egc, &ctx).await;
    assert!(egc
        .spec
        .outbound_tls
        .as_ref()
        .unwrap()
        .resolved_client_certificate
        .is_none());
    assert!(ctx.secret_ref_manager().get_refs("certs/client").contains(&owner));
    assert!(!ctx.runtime().cross_ns_ref_mgr.get_refs("certs").contains(&owner));

    for generation in ["first", "rotated"] {
        let secret = Secret {
            metadata: kube::api::ObjectMeta {
                name: Some("client".into()),
                namespace: Some("certs".into()),
                ..Default::default()
            },
            type_: Some("kubernetes.io/tls".into()),
            data: Some(BTreeMap::from([(
                "tls.crt".into(),
                ByteString(generation.as_bytes().to_vec()),
            )])),
            ..Default::default()
        };
        ctx.runtime()
            .secret_store
            .replace_all(HashMap::from([("certs/client".into(), secret.clone())]));
        egc = parse_client_identity(egc, &ctx).await;
        assert_eq!(
            egc.spec
                .outbound_tls
                .as_ref()
                .unwrap()
                .resolved_client_certificate
                .as_ref(),
            Some(&secret)
        );
    }
    let grant = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"irrelevant","namespace":"certs"},"spec":{
            "from":[{"group":"edgion.io","kind":"EdgionGatewayConfig","namespace":"default"}],
            "to":[{"group":"","kind":"Secret","name":"client"}]}}))
    .unwrap();
    ctx.runtime()
        .ref_grant_store
        .upsert("certs/irrelevant".into(), Arc::new(grant));
    egc = parse_client_identity(egc, &ctx).await;
    assert!(egc
        .spec
        .outbound_tls
        .as_ref()
        .unwrap()
        .resolved_client_certificate
        .is_some());
    ctx.runtime().ref_grant_store.remove("certs/irrelevant");
    egc = parse_client_identity(egc, &ctx).await;
    assert!(egc
        .spec
        .outbound_tls
        .as_ref()
        .unwrap()
        .resolved_client_certificate
        .is_some());
    assert!(ctx.runtime().cross_ns_ref_mgr.get_refs("certs").is_empty());

    ctx.runtime().secret_store.replace_all(HashMap::new());
    egc = parse_client_identity(egc, &ctx).await;
    assert!(egc
        .spec
        .outbound_tls
        .as_ref()
        .unwrap()
        .resolved_client_certificate
        .is_none());
    egc.spec
        .outbound_tls
        .as_mut()
        .unwrap()
        .client_certificate_ref
        .as_mut()
        .unwrap()
        .namespace = Some("other".into());
    egc = parse_client_identity(egc, &ctx).await;
    assert!(ctx.secret_ref_manager().get_refs("certs/client").is_empty());
    assert!(ctx.secret_ref_manager().get_refs("other/client").contains(&owner));
    assert!(ctx.runtime().cross_ns_ref_mgr.get_refs("other").is_empty());
    EdgionGatewayConfigHandler.on_delete(&egc, &ctx).await;
    assert!(ctx.secret_ref_manager().get_refs("other/client").is_empty());
}

#[tokio::test]
async fn global_client_identity_requires_explicit_namespace_and_core_secret_kind() {
    let ctx = make_ctx_with_secrets(HashMap::from([(
        "default/client".to_string(),
        Secret {
            metadata: kube::api::ObjectMeta {
                name: Some("client".into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            ..Default::default()
        },
    )]));
    for (namespace, group, kind, expected) in [
        (None, None, None, false),
        (Some(""), None, None, false),
        (Some("default"), None, None, true),
        (Some("default"), Some("core"), Some("Secret"), true),
        (Some("default"), Some("other.io"), Some("Secret"), false),
        (Some("default"), None, Some("ConfigMap"), false),
    ] {
        let mut egc = client_identity_config(namespace);
        let tls = egc.spec.outbound_tls.as_mut().unwrap();
        tls.resolved_client_certificate = Some(Secret::default());
        let reference = tls.client_certificate_ref.as_mut().unwrap();
        reference.group = group.map(str::to_string);
        reference.kind = kind.map(str::to_string);
        let parsed = parse_client_identity(egc, &ctx).await;
        assert_eq!(
            parsed.spec.outbound_tls.unwrap().resolved_client_certificate.is_some(),
            expected
        );
        assert_eq!(
            !ctx.secret_ref_manager().get_refs("default/client").is_empty(),
            expected
        );
    }
    let mut removed = client_identity_config(Some("default"));
    removed.spec.outbound_tls.as_mut().unwrap().client_certificate_ref = None;
    removed.spec.outbound_tls.as_mut().unwrap().resolved_client_certificate = Some(Secret::default());
    let parsed = parse_client_identity(removed, &ctx).await;
    assert!(parsed.spec.outbound_tls.unwrap().resolved_client_certificate.is_none());
    assert!(ctx.secret_ref_manager().get_refs("default/client").is_empty());
}

#[tokio::test]
async fn global_attachment_invalid_targets_register_nothing_and_status_is_intrinsic() {
    let ctx = make_ctx_with_secrets(HashMap::new());
    let handler = EdgionGatewayConfigHandler::new();
    let mut owner = EdgionGatewayConfig::new("global", Default::default());
    owner.spec.global_plugins_ref = Some(
        serde_json::from_value(
            serde_json::json!([{"name":"missing"},{"name":"bad","namespace":""},{"name":"bad/name"}]),
        )
        .unwrap(),
    );
    let errors = handler.preparse(&mut owner, &ctx);
    assert_eq!(errors.len(), 2);
    assert!(errors.iter().all(|error| error.starts_with("globalPluginsRef[")));
    let _ = handler.parse(owner.clone(), &ctx).await;
    assert_eq!(
        ctx.runtime().plugin_ref_mgr.get_refs("default/missing"),
        vec![ResourceRef::new(
            ResourceKind::EdgionGatewayConfig,
            None,
            "global".into()
        )]
    );
    assert!(ctx.runtime().plugin_ref_mgr.get_refs("/bad").is_empty());
    assert!(ctx.runtime().plugin_ref_mgr.get_refs("default/bad/name").is_empty());
    owner.metadata.namespace = Some("".into());
    let _ = handler.parse(owner.clone(), &ctx).await;
    assert!(
        ctx.runtime().plugin_ref_mgr.get_refs("default/missing").is_empty(),
        "clear old cluster owner before rejecting invalid update"
    );
    handler.on_delete(&owner, &ctx).await;
    assert!(
        ctx.runtime().cross_ns_ref_mgr.get_refs("default").is_empty(),
        "no invented Grant dependency"
    );
}

#[tokio::test]
async fn global_trust_missing_targets_register_recovery_and_retarget_clears_previous_generation() {
    let ctx = make_ctx_with_secrets(HashMap::new());
    let handler = EdgionGatewayConfigHandler::new();
    let make = |namespace: serde_json::Value| {
        serde_json::from_value::<EdgionGatewayConfig>(serde_json::json!({
        "metadata": {"name": "global-trust"}, "spec": {"realIp": {"trustedIpRefs": [{"name": "proxies", "namespace": namespace}]}}
    })).unwrap()
    };
    let mut owner = make(serde_json::json!("first"));
    assert!(
        handler.preparse(&mut owner, &ctx).is_empty(),
        "local target absence cannot invalidate shared intrinsic status"
    );
    let parsed = match handler.parse(owner, &ctx).await {
        ProcessResult::Continue(value) => value,
        ProcessResult::Skip { reason } => panic!("unexpected skip: {reason}"),
    };
    assert!(parsed.spec.real_ip.as_ref().unwrap().trusted.resolved_refs.is_some());
    assert_eq!(
        ctx.runtime().edgion_config_data_refs.get_refs("first/proxies"),
        vec!["EdgionGatewayConfig:global-trust"]
    );
    assert!(ctx.runtime().cross_ns_ref_mgr.get_refs("first").is_empty());
    let mut owner = make(serde_json::json!("second"));
    assert!(handler.preparse(&mut owner, &ctx).is_empty());
    let _ = handler.parse(owner.clone(), &ctx).await;
    assert!(ctx
        .runtime()
        .edgion_config_data_refs
        .get_refs("first/proxies")
        .is_empty());
    assert_eq!(
        ctx.runtime().edgion_config_data_refs.get_refs("second/proxies"),
        vec!["EdgionGatewayConfig:global-trust"]
    );
    owner.metadata.namespace = Some("forbidden".into());
    assert!(!handler.preparse(&mut owner, &ctx).is_empty());
    let _ = handler.parse(owner.clone(), &ctx).await;
    assert!(ctx
        .runtime()
        .edgion_config_data_refs
        .get_refs("second/proxies")
        .is_empty());
    handler.on_delete(&owner, &ctx).await;
    let mut implicit = make(serde_json::Value::Null);
    assert!(!handler.preparse(&mut implicit, &ctx).is_empty());
    let _ = handler.parse(implicit, &ctx).await;
    assert!(ctx
        .runtime()
        .edgion_config_data_refs
        .get_refs("default/proxies")
        .is_empty());
}

#[test]
fn downstream_curves_preparse_uses_shared_validation() {
    let ctx = make_ctx_with_secrets(HashMap::new());
    let mut config: EdgionGatewayConfig = serde_json::from_value(
        serde_json::json!({"metadata":{"name":"curves"},"spec":{"downstreamTls":{"ecdhCurves":["P-256","P-256"]}}}),
    )
    .unwrap();
    let errors = EdgionGatewayConfigHandler::new().preparse(&mut config, &ctx);
    assert!(errors.iter().any(|error| error.contains("duplicate curve")));
}

#[test]
fn downstream_cipher_preparse_preserves_backend_grammar_and_rejects_nul() {
    let ctx = make_ctx_with_secrets(HashMap::new());
    for (ciphers, rejected) in [
        (serde_json::json!([]), false),
        (serde_json::json!(["HIGH:!aNULL"]), false),
        (serde_json::json!(["DEFAULT\0invalid"]), true),
    ] {
        let mut config: EdgionGatewayConfig = serde_json::from_value(serde_json::json!({
            "metadata":{"name":"tls-defaults"},
            "spec":{"downstreamTls":{"minTlsVersion":"TLS1_3","ciphers":ciphers}}
        }))
        .unwrap();
        let errors = EdgionGatewayConfigHandler::new().preparse(&mut config, &ctx);
        assert_eq!(errors.iter().any(|error| error.contains("NUL")), rejected);
    }
}
