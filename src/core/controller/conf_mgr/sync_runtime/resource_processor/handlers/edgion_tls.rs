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
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::handlers::route_utils::{
    listener_allows_route_namespace, lookup_gateway,
};
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    format_secret_key, get_secret, set_parent_conditions_full, AcceptedError, HandlerContext, ProcessResult,
    ProcessorHandler, ResolvedRefsError, ResourceRef,
};
use crate::types::prelude_resources::EdgionTls;
use crate::types::resources::edgion_tls::EdgionTlsStatus;
use crate::types::resources::http_route::RouteParentStatus;
use crate::types::ResourceKind;

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

impl ProcessorHandler<EdgionTls> for EdgionTlsHandler {
    fn validate(&self, tls: &EdgionTls, _ctx: &HandlerContext) -> Vec<String> {
        let mut warnings = Vec::new();

        // Check parent_refs
        if tls.spec.parent_refs.is_none() || tls.spec.parent_refs.as_ref().map(|v| v.is_empty()).unwrap_or(true) {
            warnings.push("EdgionTls has no parent_refs, it won't be applied to any Gateway".to_string());
        }

        // Check secret existence from global secret store
        let secret_ns = tls
            .spec
            .secret_ref
            .namespace
            .as_ref()
            .or(tls.metadata.namespace.as_ref());

        if get_secret(secret_ns.map(|s| s.as_str()), &tls.spec.secret_ref.name).is_none() {
            let secret_key = format_secret_key(secret_ns, &tls.spec.secret_ref.name);
            warnings.push(format!("Secret '{}' not found (may arrive later)", secret_key));
        }

        warnings
    }

    fn parse(&self, mut tls: EdgionTls, ctx: &HandlerContext) -> ProcessResult<EdgionTls> {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionTls,
            tls.metadata.namespace.clone(),
            tls.metadata.name.clone().unwrap_or_default(),
        );

        // Clear old references first (for update scenario)
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);

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

        // Try to resolve Secret from global store
        if let Some(secret) = get_secret(secret_ns.map(|s| s.as_str()), &tls.spec.secret_ref.name) {
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

                // Try to resolve CA Secret from global store
                if let Some(ca_secret) = get_secret(ca_ns.map(|s| s.as_str()), &ca_secret_ref.name) {
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

        // 3. Resolve ports from parentRefs → Gateway → listener.port
        //
        // Per Gateway API spec:
        //   - parentRef.port set → use that port directly
        //   - parentRef.sectionName set → find matching listener by name, use its port
        //   - neither set → attach to ALL listeners of the parent Gateway
        //
        // Only include ports from listeners whose allowedRoutes policy permits
        // this EdgionTls's namespace (default: Same).
        let resource_ns = tls.metadata.namespace.as_deref().unwrap_or("default");
        if let Some(parent_refs) = &tls.spec.parent_refs {
            let mut ports = Vec::new();
            for parent_ref in parent_refs {
                if let Some(port) = parent_ref.port {
                    ports.push(port as u16);
                } else {
                    let gw_ns = parent_ref
                        .namespace
                        .as_deref()
                        .or(tls.metadata.namespace.as_deref())
                        .unwrap_or("default");
                    if let Some(gateway) = lookup_gateway(gw_ns, &parent_ref.name) {
                        if let Some(listeners) = &gateway.spec.listeners {
                            if let Some(section_name) = &parent_ref.section_name {
                                if let Some(listener) = listeners.iter().find(|l| l.name == *section_name) {
                                    if listener_allows_route_namespace(
                                        &listener.allowed_routes,
                                        resource_ns,
                                        gw_ns,
                                    ) {
                                        ports.push(listener.port as u16);
                                    }
                                }
                            } else {
                                for listener in listeners {
                                    if listener_allows_route_namespace(
                                        &listener.allowed_routes,
                                        resource_ns,
                                        gw_ns,
                                    ) {
                                        ports.push(listener.port as u16);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if !ports.is_empty() {
                ports.sort_unstable();
                ports.dedup();
                tracing::debug!(
                    edgion_tls = %resource_ref.key(),
                    resolved_ports = ?ports,
                    "Resolved ports from parentRefs"
                );
                tls.spec.resolved_ports = Some(ports);
            }
        }

        ProcessResult::Continue(tls)
    }

    fn on_change(&self, tls: &EdgionTls, _ctx: &HandlerContext) {
        let tls_ns = tls.metadata.namespace.as_deref().unwrap_or("default");
        let tls_name = tls.metadata.name.as_deref().unwrap_or("");

        // Register in gateway_route_index so Gateway's on_change can requeue
        // this EdgionTls when listener hostnames or ports change. Without this,
        // an EdgionTls processed before its Gateway would permanently have
        // resolved_ports = None with no mechanism to trigger re-resolution.
        update_gateway_route_index(
            ResourceKind::EdgionTls,
            tls_ns,
            tls_name,
            tls.spec.parent_refs.as_ref(),
        );
    }

    fn on_delete(&self, tls: &EdgionTls, ctx: &HandlerContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionTls,
            tls.metadata.namespace.clone(),
            tls.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);

        let tls_ns = tls.metadata.namespace.as_deref().unwrap_or("default");
        let tls_name = tls.metadata.name.as_deref().unwrap_or("");
        remove_from_gateway_route_index(ResourceKind::EdgionTls, tls_ns, tls_name);

        tracing::debug!(
            edgion_tls = %resource_ref.key(),
            "Cleared secret and gateway_route_index references on EdgionTls delete"
        );
    }

    fn update_status(&self, tls: &mut EdgionTls, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = tls.metadata.generation;
        let resource_ns = tls.metadata.namespace.as_deref().unwrap_or("default");

        let validation_accepted_errors = AcceptedError::from_validation_errors(validation_errors);

        // Check Secret resolution for ResolvedRefs condition
        let secret_ns = tls
            .spec
            .secret_ref
            .namespace
            .as_ref()
            .or(tls.metadata.namespace.as_ref());
        let secret_resolved = get_secret(secret_ns.map(|s| s.as_str()), &tls.spec.secret_ref.name).is_some();

        let resolved_refs_errors: Vec<ResolvedRefsError> = if secret_resolved {
            vec![]
        } else {
            vec![ResolvedRefsError::SecretNotFound {
                namespace: secret_ns.map(|s| s.as_str()).unwrap_or("default").to_string(),
                name: tls.spec.secret_ref.name.clone(),
            }]
        };

        let status = tls.status.get_or_insert_with(|| EdgionTlsStatus { parents: vec![] });

        if let Some(parent_refs) = &tls.spec.parent_refs {
            for parent_ref in parent_refs {
                let mut accepted_errors = super::route_utils::validate_parent_ref_accepted(
                    resource_ns,
                    parent_ref,
                    None,
                );
                accepted_errors.extend(validation_accepted_errors.clone());

                let parent_status = status.parents.iter_mut().find(|ps| {
                    ps.parent_ref.name == parent_ref.name
                        && ps.parent_ref.namespace == parent_ref.namespace
                        && ps.parent_ref.section_name == parent_ref.section_name
                });

                if let Some(ps) = parent_status {
                    set_parent_conditions_full(
                        &mut ps.conditions,
                        &accepted_errors,
                        &resolved_refs_errors,
                        generation,
                    );
                } else {
                    let mut conditions = Vec::new();
                    set_parent_conditions_full(
                        &mut conditions,
                        &accepted_errors,
                        &resolved_refs_errors,
                        generation,
                    );

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
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::SecretRefManager;
    use crate::core::controller::conf_mgr::sync_runtime::workqueue::TriggerChain;
    use crate::types::resources::common::ParentReference;
    use crate::types::resources::gateway::SecretObjectReference;
    use kube::api::ObjectMeta;
    use std::sync::Arc;

    fn make_ctx() -> HandlerContext {
        HandlerContext::new(
            Arc::new(SecretRefManager::new()),
            None,
            None,
            TriggerChain::default(),
            3,
        )
    }

    fn make_tls(parent_refs: Option<Vec<ParentReference>>, secret_name: &str) -> EdgionTls {
        EdgionTls {
            metadata: ObjectMeta {
                name: Some("test-tls".to_string()),
                namespace: Some("test-ns".to_string()),
                generation: Some(1),
                ..Default::default()
            },
            spec: crate::types::resources::edgion_tls::EdgionTlsSpec {
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
                extend: None,
                secret: None,
                resolved_ports: None,
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
        let ctx = make_ctx();
        let parent_refs = vec![make_parent_ref("my-gateway", "test-ns")];
        let mut tls = make_tls(Some(parent_refs), "missing-secret");

        let errors = vec!["Secret 'test-ns/missing-secret' not found (may arrive later)".to_string()];
        handler.update_status(&mut tls, &ctx, &errors);

        let status = tls.status.as_ref().expect("status should be set");
        assert_eq!(status.parents.len(), 1);

        let conditions = &status.parents[0].conditions;
        let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(accepted.status, "False", "Accepted should be False when validation errors exist");
        assert_eq!(accepted.reason, "Invalid");
        assert!(accepted.message.contains("Secret 'test-ns/missing-secret' not found"));
    }

    #[test]
    fn test_update_status_no_errors_sets_accepted_true() {
        let handler = EdgionTlsHandler::default();
        let ctx = make_ctx();
        let parent_refs = vec![make_parent_ref("my-gateway", "test-ns")];
        let mut tls = make_tls(Some(parent_refs), "existing-secret");

        handler.update_status(&mut tls, &ctx, &[]);

        let status = tls.status.as_ref().expect("status should be set");
        assert_eq!(status.parents.len(), 1);

        let conditions = &status.parents[0].conditions;
        let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(accepted.status, "True");

        let resolved = conditions.iter().find(|c| c.type_ == "ResolvedRefs").unwrap();
        assert_eq!(resolved.status, "False", "Secret not in global store so ResolvedRefs should be False");
    }

    #[test]
    fn test_update_status_no_parent_refs_still_initializes_status() {
        let handler = EdgionTlsHandler::default();
        let ctx = make_ctx();
        let mut tls = make_tls(None, "some-secret");

        let errors = vec!["EdgionTls has no parent_refs".to_string()];
        handler.update_status(&mut tls, &ctx, &errors);

        let status = tls.status.as_ref().expect("status should be initialized");
        assert!(status.parents.is_empty(), "no parent_refs means no per-parent status entries");
    }

    #[test]
    fn test_update_status_multiple_parents_all_get_validation_errors() {
        let handler = EdgionTlsHandler::default();
        let ctx = make_ctx();
        let parent_refs = vec![
            make_parent_ref("gw-1", "test-ns"),
            make_parent_ref("gw-2", "test-ns"),
        ];
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
        let ctx = make_ctx();
        let parent_refs = vec![make_parent_ref("my-gw", "test-ns")];
        let mut tls = make_tls(Some(parent_refs.clone()), "test-secret");

        handler.update_status(&mut tls, &ctx, &[]);
        let accepted = &tls.status.as_ref().unwrap().parents[0]
            .conditions.iter().find(|c| c.type_ == "Accepted").unwrap().status;
        assert_eq!(accepted, "True");

        let errors = vec!["Secret expired".to_string()];
        handler.update_status(&mut tls, &ctx, &errors);

        let status = tls.status.as_ref().unwrap();
        assert_eq!(status.parents.len(), 1, "should update in-place, not duplicate");
        let accepted = status.parents[0].conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
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
    fn test_update_status_accepted_false_also_sets_programmed_ready_false() {
        let handler = EdgionTlsHandler::default();
        let ctx = make_ctx();
        let parent_refs = vec![make_parent_ref("my-gw", "test-ns")];
        let mut tls = make_tls(Some(parent_refs), "bad-secret");

        handler.update_status(&mut tls, &ctx, &[]);
        let conditions = &tls.status.as_ref().unwrap().parents[0].conditions;
        let programmed = conditions.iter().find(|c| c.type_ == "Programmed").unwrap();
        assert_eq!(programmed.status, "True");

        let errors = vec!["Secret missing".to_string()];
        handler.update_status(&mut tls, &ctx, &errors);

        let conditions = &tls.status.as_ref().unwrap().parents[0].conditions;
        let accepted = conditions.iter().find(|c| c.type_ == "Accepted").unwrap();
        assert_eq!(accepted.status, "False");

        let programmed = conditions.iter().find(|c| c.type_ == "Programmed").unwrap();
        assert_eq!(programmed.status, "False", "Programmed must be False when Accepted is False");

        let ready = conditions.iter().find(|c| c.type_ == "Ready").unwrap();
        assert_eq!(ready.status, "False", "Ready must be False when Accepted is False");
    }
}
