//! EdgionGatewayConfig Handler
//!
//! Handles EdgionGatewayConfig resources with Gateway API standard status management.

use crate::conf_mgr::sync_runtime::resource_processor::credential::{resolve_ca_certificate_refs, CaFetchStrategy};
use crate::conf_mgr::sync_runtime::resource_processor::{
    accepted_condition, condition_false, condition_types, update_condition, HandlerContext, ProcessResult,
    ProcessorHandler, ResourceRef,
};
use edgion_resources::prelude_resources::{EdgionGatewayConfig, EdgionPlugins};
use edgion_resources::resources::edgion_gateway_config::EdgionGatewayConfigStatus;
use edgion_resources::ResourceKind;

use super::san_guard;

/// Requeue every GatewayClass so its ResolvedRefs/Accepted conditions are recomputed after an
/// EdgionGatewayConfig change or delete. GatewayClass cardinality is tiny, so a brute-force
/// requeue-all is cheap and avoids a dedicated bidirectional ref-tracking subsystem. `ctx.requeue`
/// carries the trigger chain (cycle/depth safe); GatewayClass requeues nothing, so the edge is
/// terminal.
async fn requeue_all_gateway_classes(ctx: &HandlerContext) {
    let Some(gc_processor) = ctx.runtime().processor_registry.get("GatewayClass") else {
        return;
    };
    for key in gc_processor.list_keys() {
        ctx.requeue("GatewayClass", key).await;
    }
}

/// EdgionGatewayConfig handler
pub struct EdgionGatewayConfigHandler;

impl EdgionGatewayConfigHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EdgionGatewayConfigHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<EdgionGatewayConfig> for EdgionGatewayConfigHandler {
    fn preparse(&self, egc: &mut EdgionGatewayConfig, ctx: &HandlerContext) -> Vec<String> {
        // Phase 1 guard: reject outbound TLS subjectAltNames/hostname on
        // EdgionGatewayConfig where SAN pinning enforcement is not yet implemented.
        let mut errors = san_guard::check_egc_san(egc);
        // dnsResolver `linkSysRef` and `servers` are mutually exclusive (the
        // structural CRD cannot express `oneOf`, so it is enforced here).
        errors.extend(egc.spec.validate_dns_resolver());
        errors.extend(egc.spec.validate_link_sys());
        errors.extend(egc.spec.validate_request_body());
        errors.extend(egc.spec.validate_forwarded_headers());
        errors.extend(egc.spec.validate_real_ip());
        errors.extend(egc.spec.validate_preflight_policy());
        errors.extend(egc.spec.validate_plugin_policy());
        if let Some(processor) = ctx.runtime().processor_registry.get("EdgionPlugins") {
            for reference in egc.spec.global_plugins_ref.as_deref().unwrap_or_default() {
                let namespace = reference.namespace.as_deref().unwrap_or("default");
                let key = format!("{namespace}/{}", reference.name);
                if let Some(plugins) = processor
                    .get_raw_any(&key)
                    .and_then(|raw| raw.downcast::<EdgionPlugins>().ok())
                {
                    errors.extend(
                        plugins
                            .validate_request_id_early_attachment()
                            .into_iter()
                            .map(|error| format!("globalPluginsRef {key}: {error}")),
                    );
                }
            }
        }
        errors
    }

    async fn parse(&self, mut egc: EdgionGatewayConfig, ctx: &HandlerContext) -> ProcessResult<EdgionGatewayConfig> {
        egc.spec.check_deprecated_fields();

        // Resolve the cluster-wide outbound CA Secret (validation.caCertificateRefs) and
        // register the reference so a Secret change requeues this EGC. EGC is
        // cluster-scoped, so `metadata.namespace` is None; the referenced Secret
        // SHOULD therefore carry an explicit `namespace` in `validation.caCertificateRefs[]`.
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionGatewayConfig,
            egc.metadata.namespace.clone(),
            egc.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);
        ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&resource_ref);
        ctx.runtime().plugin_ref_mgr.clear_resource_refs(&resource_ref);
        for reference in egc.spec.global_plugins_ref.as_deref().unwrap_or_default() {
            let namespace = reference.namespace.as_deref().unwrap_or("default");
            ctx.runtime()
                .plugin_ref_mgr
                .add_ref(format!("{namespace}/{}", reference.name), resource_ref.clone());
        }

        if let Some(ref mut ob) = egc.spec.outbound_tls {
            // Reset the runtime-resolved field first to avoid stale values on update.
            ob.resolved_ca_certificates = None;

            // Resolve cluster-wide outbound CA certificate refs.
            // EGC is cluster-scoped (owner_ns = None); each ref must carry an explicit
            // namespace or it is skipped as unresolvable by the helper.
            let refs = ob
                .validation
                .as_ref()
                .and_then(|v| v.ca_certificate_refs.as_deref())
                .unwrap_or_default();
            if !refs.is_empty() {
                let resolved = resolve_ca_certificate_refs(
                    ctx,
                    &resource_ref,
                    None, // cluster-scoped: no owner namespace
                    refs,
                    CaFetchStrategy::GrantGated {
                        group: "edgion.io",
                        kind: "EdgionGatewayConfig",
                    },
                );
                ob.resolved_ca_certificates = (!resolved.is_empty()).then_some(resolved);
            }
        }

        ProcessResult::Continue(egc)
    }

    async fn on_change(&self, _egc: &EdgionGatewayConfig, ctx: &HandlerContext) {
        // A GatewayClass's ResolvedRefs/Accepted conditions depend on whether the
        // EdgionGatewayConfig it references exists, so recompute them on every EGC change.
        requeue_all_gateway_classes(ctx).await;
    }

    async fn on_delete(&self, egc: &EdgionGatewayConfig, ctx: &HandlerContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionGatewayConfig,
            egc.metadata.namespace.clone(),
            egc.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);
        ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&resource_ref);
        ctx.runtime().plugin_ref_mgr.clear_resource_refs(&resource_ref);
        // A GatewayClass's ResolvedRefs/Accepted conditions depend on whether the
        // EdgionGatewayConfig it references exists, so recompute them on every EGC delete.
        requeue_all_gateway_classes(ctx).await;
    }

    fn update_status(&self, egc: &mut EdgionGatewayConfig, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = egc.metadata.generation;

        let status = egc
            .status
            .get_or_insert_with(|| EdgionGatewayConfigStatus { conditions: vec![] });

        if validation_errors.is_empty() {
            update_condition(&mut status.conditions, accepted_condition(generation));
        } else {
            update_condition(
                &mut status.conditions,
                condition_false(
                    condition_types::ACCEPTED,
                    "Invalid",
                    validation_errors.join("; "),
                    generation,
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::ControllerRuntime;
    use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
    use edgion_resources::resources::common::{OutboundTlsConfig, OutboundTlsValidation};
    use edgion_resources::resources::edgion_gateway_config::EdgionGatewayConfigSpec;
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

    #[tokio::test]
    async fn global_request_id_validation_observes_replaced_raw_generation() {
        use crate::conf_mgr::sync_runtime::resource_processor::handlers::EdgionPluginsHandler;
        use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
        use edgion_resources::resources::edgion_gateway_config::PluginReference;

        let runtime = Arc::new(ControllerRuntime::default());
        let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, runtime.clone());
        let processor = Arc::new(ResourceProcessor::<EdgionPlugins>::new(
            "EdgionPlugins",
            16,
            Arc::new(EdgionPluginsHandler::new()),
            runtime,
        ));
        let raw = Arc::new(LocalRawStore::new());
        processor.set_raw_store(raw.clone());
        ctx.runtime().processor_registry.register(processor);
        let handler = EdgionGatewayConfigHandler::new();
        let mut egc = EdgionGatewayConfig::new("global", EdgionGatewayConfigSpec::default());
        egc.spec.global_plugins_ref = Some(vec![PluginReference {
            name: "ids".to_string(),
            namespace: None,
        }]);
        for source in ["traceId", "ctx", "traceId"] {
            let source = if source == "ctx" {
                serde_json::json!({"type": "ctx", "name": "business"})
            } else {
                serde_json::json!({"type": source})
            };
            let invalid = source["type"] == "ctx";
            let plugins: EdgionPlugins = serde_json::from_value(serde_json::json!({
                "apiVersion": "edgion.io/v1", "kind": "EdgionPlugins",
                "metadata": {"namespace": "default", "name": "ids"},
                "spec": {"requestPlugins": [{"type": "RequestId", "config": {
                    "ids": [{"header": "x-business-id", "source": source}]
                }}]}
            }))
            .unwrap();
            raw.upsert("default/ids".to_string(), Arc::new(plugins));
            let errors = handler.preparse(&mut egc, &ctx);
            assert_eq!(!errors.is_empty(), invalid, "{errors:?}");
        }
    }

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
            data: Some(data),
            type_: Some("kubernetes.io/tls".to_string()),
            ..Default::default()
        };
        let mut secrets = HashMap::new();
        // EGC is cluster-scoped; owner namespace falls back to "default".
        secrets.insert("default/global-ca".to_string(), ca_secret);

        let ctx = make_ctx_with_secrets(secrets);

        let spec = EdgionGatewayConfigSpec {
            outbound_tls: Some(OutboundTlsConfig {
                validation: Some(OutboundTlsValidation {
                    ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
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
            data: Some(data),
            type_: Some("kubernetes.io/tls".to_string()),
            ..Default::default()
        };
        let mut secrets = HashMap::new();
        secrets.insert("default/global-ca".to_string(), ca_secret);

        let ctx = make_ctx_with_secrets(secrets);

        let spec = EdgionGatewayConfigSpec {
            outbound_tls: Some(OutboundTlsConfig {
                validation: Some(OutboundTlsValidation {
                    ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
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
            outbound_tls: Some(OutboundTlsConfig {
                validation: Some(OutboundTlsValidation {
                    ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
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
        assert!(ctx
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
        assert!(ctx
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
            request_body: edgion_resources::resources::edgion_gateway_config::RequestBodyConfig {
                max_body_size: Some("garbage".to_string()),
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
        use edgion_resources::resources::edgion_config_data::IpGroup;
        use edgion_resources::resources::edgion_plugins::RealIpConfig;

        let ctx = make_ctx_with_secrets(HashMap::new());
        let spec = EdgionGatewayConfigSpec {
            real_ip: Some(RealIpConfig {
                trusted_ips: vec![IpGroup {
                    name: "internal".to_string(),
                    description: None,
                    cidrs: vec!["10.0.0.0/8".to_string()],
                }],
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
            outbound_tls: Some(OutboundTlsConfig {
                validation: Some(OutboundTlsValidation {
                    ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
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
}
