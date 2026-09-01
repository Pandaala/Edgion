//! EdgionTls Handler
//!
//! Handles EdgionTls resources with:
//! - Validation (warnings for missing secrets, parent_refs, etc.)
//! - Server Secret reference resolution (secret_ref -> spec.secret)
//! - CA Secret reference resolution for mTLS (client_auth.ca_secret_ref -> client_auth.ca_secret)
//! - SecretRefManager registration for cascading updates
//! - Gateway route index registration for port revalidation on Gateway changes
//! - Gateway API standard status management (per parent)

use super::{remove_from_gateway_route_index, update_gateway_route_index};
use crate::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key, set_parent_conditions_full, AcceptedError, HandlerContext, ProcessResult, ProcessorHandler,
    ResolvedRefsError, ResourceRef,
};
use edgion_resources::prelude_resources::EdgionTls;
use edgion_resources::resources::common::SecretObjectReference;
use edgion_resources::resources::edgion_tls::EdgionTlsStatus;
use edgion_resources::resources::http_route::RouteParentStatus;
use edgion_resources::ResourceKind;

/// Returns `true` when `secret_ref` points at a different namespace than `owner_ns`,
/// ReferenceGrant validation is enabled, and no `ReferenceGrant` authorizes the
/// `EdgionTls` -> `Secret` reference. Mirrors the cross-namespace guard used by
/// `GatewayHandler` for `certificateRefs`; when validation is disabled this always
/// returns `false`, keeping behavior consistent with the other handlers.
fn cross_ns_secret_ref_denied(ctx: &HandlerContext, owner_ns: &str, secret_ref: &SecretObjectReference) -> bool {
    if !secret_ref.is_cross_namespace(owner_ns) || !crate::config::is_reference_grant_validation_enabled() {
        return false;
    }
    let target_ns = secret_ref.effective_namespace(owner_ns);
    !ctx.runtime().ref_grant_store.check_reference_allowed(
        owner_ns,
        "edgion.io",
        "EdgionTls",
        target_ns,
        "",
        "Secret",
        Some(&secret_ref.name),
    )
}

/// EdgionTls handler
///
/// Features:
/// - validate: Check for parent_refs and secret existence
/// - parse: Parse secret_ref -> fill spec.secret
/// - parse: Parse client_auth.ca_secret_ref -> fill client_auth.ca_secret
/// - parse: Register Secret references to SecretRefManager
/// - on_change: Register in gateway_route_index so Gateway port/hostname changes trigger requeue
/// - on_delete: Clear SecretRefManager and gateway_route_index references
pub struct EdgionTlsHandler {
    controller_name: String,
}

impl EdgionTlsHandler {
    pub fn new(controller_name: String) -> Self {
        Self { controller_name }
    }
}

impl Default for EdgionTlsHandler {
    fn default() -> Self {
        Self::new("edgion.io/gateway-controller".to_string())
    }
}

#[async_trait::async_trait]
impl ProcessorHandler<EdgionTls> for EdgionTlsHandler {
    fn validate(&self, tls: &EdgionTls, ctx: &HandlerContext) -> Vec<String> {
        let mut warnings = Vec::new();

        // Check parent_refs
        if tls.spec.parent_refs.is_none() || tls.spec.parent_refs.as_ref().map(|v| v.is_empty()).unwrap_or(true) {
            warnings.push("EdgionTls has no parent_refs, it won't be applied to any Gateway".to_string());
        }

        // Check secret existence from global secret store. Cert-content checks
        // (expiry, SAN, key match) are deferred to update_status() via
        // validate_cert() and surfaced as ResolvedRefs/InvalidCertificate so
        // they appear on the per-parent status condition rather than as a generic
        // Accepted/Invalid warning.
        let secret_ns = tls
            .spec
            .secret_ref
            .namespace
            .as_ref()
            .or(tls.metadata.namespace.as_ref());

        if ctx
            .runtime()
            .secret_store
            .get(secret_ns.map(|s| s.as_str()), &tls.spec.secret_ref.name)
            .is_none()
        {
            let secret_key = format_secret_key(secret_ns, &tls.spec.secret_ref.name);
            warnings.push(format!("Secret '{}' not found (may arrive later)", secret_key));
        }

        warnings
    }

    async fn parse(&self, mut tls: EdgionTls, ctx: &HandlerContext) -> ProcessResult<EdgionTls> {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionTls,
            tls.metadata.namespace.clone(),
            tls.metadata.name.clone().unwrap_or_default(),
        );

        // Clear old references first (for update scenario)
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);
        // Clear old cross-namespace references too; they are re-registered below so a
        // later ReferenceGrant add/remove requeues this EdgionTls (mirrors GatewayHandler).
        ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&resource_ref);

        // 1. Resolve server Secret from global secret store
        let secret_ns = tls
            .spec
            .secret_ref
            .namespace
            .as_ref()
            .or(tls.metadata.namespace.as_ref());
        let secret_key = format_secret_key(secret_ns, &tls.spec.secret_ref.name);

        // Register reference relationship (critical for cascading updates)
        // When this Secret arrives or updates, SecretHandler will trigger requeue for this EdgionTls
        ctx.secret_ref_manager()
            .add_ref(secret_key.clone(), resource_ref.clone());

        // Enforce ReferenceGrant for cross-namespace Secret refs before reading the
        // Secret, mirroring GatewayHandler's certificateRefs handling (gateway.rs).
        // The controller ServiceAccount can read Secrets cluster-wide, so this is the
        // only tenant-isolation boundary preventing a low-privilege EdgionTls writer
        // from embedding another namespace's TLS private key into spec.secret.
        let owner_ns = tls.metadata.namespace.as_deref().unwrap_or("default");
        // Track the cross-namespace dependency so ReferenceGrant changes in the target
        // namespace requeue this EdgionTls (covers both grant-added and grant-removed).
        if tls.spec.secret_ref.is_cross_namespace(owner_ns) {
            ctx.runtime().cross_ns_ref_mgr.add_ref(
                tls.spec.secret_ref.effective_namespace(owner_ns).to_string(),
                resource_ref.clone(),
            );
        }
        if cross_ns_secret_ref_denied(ctx, owner_ns, &tls.spec.secret_ref) {
            tracing::info!(
                edgion_tls = %resource_ref.key(),
                secret_key = %secret_key,
                "Cross-namespace Secret reference denied (no ReferenceGrant); leaving secret unresolved"
            );
        } else if let Some(secret) = ctx
            .runtime()
            .secret_store
            .get(secret_ns.map(|s| s.as_str()), &tls.spec.secret_ref.name)
        {
            tls.spec.secret = Some(secret);
            tracing::debug!(
                edgion_tls = %resource_ref.key(),
                secret_key = %secret_key,
                "Secret resolved and filled into EdgionTls"
            );
        } else {
            // Secret not found yet - this is normal if Secret arrives after EdgionTls
            // The SecretRefManager reference ensures we'll be requeued when Secret arrives
            tracing::info!(
                edgion_tls = %resource_ref.key(),
                secret_key = %secret_key,
                "Secret not found yet, will be reprocessed when Secret arrives"
            );
        }

        // 2. Resolve CA Secret (if mTLS is configured)
        if let Some(ref mut client_auth) = tls.spec.client_auth {
            if let Some(ref ca_secret_ref) = client_auth.ca_secret_ref {
                let ca_ns = ca_secret_ref.namespace.as_ref().or(tls.metadata.namespace.as_ref());
                let ca_secret_key = format_secret_key(ca_ns, &ca_secret_ref.name);

                // Register CA Secret reference for cascading updates
                ctx.secret_ref_manager()
                    .add_ref(ca_secret_key.clone(), resource_ref.clone());

                // Same cross-namespace ReferenceGrant guard as the server cert above:
                // the mTLS CA Secret can also carry private key material.
                let ca_owner_ns = tls.metadata.namespace.as_deref().unwrap_or("default");
                if ca_secret_ref.is_cross_namespace(ca_owner_ns) {
                    ctx.runtime().cross_ns_ref_mgr.add_ref(
                        ca_secret_ref.effective_namespace(ca_owner_ns).to_string(),
                        resource_ref.clone(),
                    );
                }
                if cross_ns_secret_ref_denied(ctx, ca_owner_ns, ca_secret_ref) {
                    tracing::info!(
                        edgion_tls = %resource_ref.key(),
                        ca_secret_key = %ca_secret_key,
                        "Cross-namespace CA Secret reference denied (no ReferenceGrant); leaving ca_secret unresolved"
                    );
                } else if let Some(ca_secret) = ctx
                    .runtime()
                    .secret_store
                    .get(ca_ns.map(|s| s.as_str()), &ca_secret_ref.name)
                {
                    client_auth.ca_secret = Some(ca_secret);
                    tracing::debug!(
                        edgion_tls = %resource_ref.key(),
                        ca_secret_key = %ca_secret_key,
                        "CA Secret resolved and filled into EdgionTls.client_auth"
                    );
                } else {
                    tracing::info!(
                        edgion_tls = %resource_ref.key(),
                        ca_secret_key = %ca_secret_key,
                        "CA Secret not found yet, mTLS will be enabled when Secret arrives"
                    );
                }
            }
        }

        // 3. Resolve listener keys from parentRefs for per-listener route isolation.
        // Defensive filter: EdgionTls only makes sense on HTTPS/TLS listeners.
        // A Gateway is not expected to mix TLS-capable and non-TLS listeners, but
        // we keep this local check so the resolved spec stays clean even if upstream
        // Gateway validation ever lets a mixed configuration through.
        let resource_ns = tls.metadata.namespace.as_deref().unwrap_or("default");
        tls.spec.resolved_listeners = None;
        if let Some(parent_refs) = &tls.spec.parent_refs {
            let listener_keys: Vec<_> = super::route_utils::resolve_listener_keys(
                parent_refs,
                resource_ns,
                tls.metadata.namespace.as_deref(),
                ctx.runtime(),
            )
            .into_iter()
            .filter(|k| matches!(k.protocol(), "HTTPS" | "TLS"))
            .collect();
            if !listener_keys.is_empty() {
                tracing::debug!(
                    edgion_tls = %resource_ref.key(),
                    resolved_listeners = ?listener_keys,
                    "Resolved listeners from parentRefs"
                );
                tls.spec.resolved_listeners = Some(listener_keys);
            }
        }

        ProcessResult::Continue(tls)
    }

    async fn on_change(&self, tls: &EdgionTls, ctx: &HandlerContext) {
        let tls_ns = tls.metadata.namespace.as_deref().unwrap_or("default");
        let tls_name = tls.metadata.name.as_deref().unwrap_or("");

        // Register in gateway_route_index so Gateway's on_change can requeue
        // this EdgionTls when listener hostnames or ports change. Without this,
        // an EdgionTls processed before its Gateway would permanently have
        // resolved_listeners = None with no mechanism to trigger re-resolution.
        //
        // Also requeue Gateways this EdgionTls no longer references: a coalesced
        // delete->recreate that re-points parent_refs skips handler.on_delete for
        // the previous generation, so the dropped Gateway would otherwise never
        // be told this EdgionTls detached from it.
        let dropped_gateways = update_gateway_route_index(
            ResourceKind::EdgionTls,
            tls_ns,
            tls_name,
            tls.spec.parent_refs.as_ref(),
            ctx.runtime(),
        );
        for gateway_key in dropped_gateways {
            ctx.requeue("Gateway", gateway_key).await;
        }
    }

    async fn on_delete(&self, tls: &EdgionTls, ctx: &HandlerContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionTls,
            tls.metadata.namespace.clone(),
            tls.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);
        ctx.runtime().cross_ns_ref_mgr.clear_resource_refs(&resource_ref);

        let tls_ns = tls.metadata.namespace.as_deref().unwrap_or("default");
        let tls_name = tls.metadata.name.as_deref().unwrap_or("");
        remove_from_gateway_route_index(ResourceKind::EdgionTls, tls_ns, tls_name, ctx.runtime());

        tracing::debug!(
            edgion_tls = %resource_ref.key(),
            "Cleared secret and gateway_route_index references on EdgionTls delete"
        );
    }

    fn update_status(&self, tls: &mut EdgionTls, ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = tls.metadata.generation;
        let resource_ns = tls.metadata.namespace.as_deref().unwrap_or("default");

        let validation_accepted_errors = AcceptedError::from_validation_errors(validation_errors);

        // parse() already resolved secret_ref -> spec.secret; reuse that result
        // instead of querying secret_store a second time.
        let resolved_refs_errors: Vec<ResolvedRefsError> = if tls.spec.secret.is_some() {
            // Secret resolved: run the SAME validation the gateway runs and surface
            // cert-CONTENT failures on ResolvedRefs. Filter out the not-found variants
            // (secret/cert/key absent) — those are owned by the secret-absent branch
            // below; folding them here would double-report. status_message() is
            // deterministic (no wall-clock) so the message is byte-stable across
            // reconciles (avoids the status-write hot loop).
            use edgion_common::tls_validation::CertValidationError as CVE;
            edgion_common::tls_validation::validate_cert(tls)
                .errors
                .into_iter()
                .filter(|e| !matches!(e, CVE::SecretNotFound | CVE::CertNotFound | CVE::KeyNotFound))
                .map(|e| ResolvedRefsError::CertContentInvalid {
                    detail: e.status_message(),
                })
                .collect()
        } else {
            let secret_ns = tls
                .spec
                .secret_ref
                .namespace
                .as_ref()
                .or(tls.metadata.namespace.as_ref());
            let target_ns = secret_ns.map(|s| s.as_str()).unwrap_or("default").to_string();
            // Distinguish "denied by ReferenceGrant" from "Secret simply absent" so
            // the K8s ResolvedRefs condition reports RefNotPermitted instead of a
            // perpetual SecretNotFound wait when the cross-ns ref was rejected in parse().
            if cross_ns_secret_ref_denied(ctx, resource_ns, &tls.spec.secret_ref) {
                vec![ResolvedRefsError::RefNotPermitted {
                    from_namespace: resource_ns.to_string(),
                    from_kind: "EdgionTls",
                    target_namespace: target_ns,
                    target_name: tls.spec.secret_ref.name.clone(),
                }]
            } else {
                vec![ResolvedRefsError::SecretNotFound {
                    namespace: target_ns,
                    name: tls.spec.secret_ref.name.clone(),
                }]
            }
        };

        let status = tls.status.get_or_insert_with(|| EdgionTlsStatus { parents: vec![] });

        if let Some(parent_refs) = &tls.spec.parent_refs {
            for parent_ref in parent_refs {
                let accepted_result = super::route_utils::validate_parent_ref_accepted(
                    resource_ns,
                    parent_ref,
                    None,
                    "EdgionTls",
                    ctx.runtime(),
                );
                let mut accepted_errors = accepted_result.errors;

                // EdgionTls only makes sense on HTTPS/TLS listeners — reject non-TLS ones.
                if accepted_errors.is_empty()
                    && !accepted_result.matched_listener_protocols.is_empty()
                    && !accepted_result
                        .matched_listener_protocols
                        .iter()
                        .any(|p| p == "HTTPS" || p == "TLS")
                {
                    accepted_errors.push(AcceptedError::Invalid {
                        message: "EdgionTls requires HTTPS or TLS listener".to_string(),
                    });
                }

                accepted_errors.extend(validation_accepted_errors.clone());

                let parent_status = status.parents.iter_mut().find(|ps| {
                    ps.parent_ref.name == parent_ref.name
                        && ps.parent_ref.namespace == parent_ref.namespace
                        && ps.parent_ref.section_name == parent_ref.section_name
                });

                if let Some(ps) = parent_status {
                    set_parent_conditions_full(&mut ps.conditions, &accepted_errors, &resolved_refs_errors, generation);
                } else {
                    let mut conditions = Vec::new();
                    set_parent_conditions_full(&mut conditions, &accepted_errors, &resolved_refs_errors, generation);

                    status.parents.push(RouteParentStatus {
                        parent_ref: parent_ref.clone(),
                        controller_name: self.controller_name.clone(),
                        conditions,
                    });
                }
            }

            super::route_utils::retain_current_parent_statuses(&mut status.parents, parent_refs);
        } else {
            status.parents.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::{LocalRawStore, ResourceProcessor};
    use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use edgion_resources::prelude_resources::Gateway;
    use edgion_resources::resources::common::ParentReference;
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
                resolved_inbound_proxy_protocol: Default::default(),
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
                resolved_log_labels: None,
            },
            status: None,
        }
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
}
