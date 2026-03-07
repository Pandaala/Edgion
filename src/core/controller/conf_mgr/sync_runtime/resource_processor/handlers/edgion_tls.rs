//! EdgionTls Handler
//!
//! Handles EdgionTls resources with:
//! - Validation (warnings for missing secrets, parent_refs, etc.)
//! - Server Secret reference resolution (secret_ref -> spec.secret)
//! - CA Secret reference resolution for mTLS (client_auth.ca_secret_ref -> client_auth.ca_secret)
//! - SecretRefManager registration for cascading updates
//! - Gateway API standard status management (per parent)

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::handlers::route_utils::lookup_gateway;
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::{
    accepted_condition, condition_false, condition_reasons, condition_true, condition_types, format_secret_key,
    get_secret, programmed_condition, ready_condition, update_condition, HandlerContext, ProcessResult,
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
/// - on_delete: Clear SecretRefManager references
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
        if let Some(parent_refs) = &tls.spec.parent_refs {
            let mut ports = Vec::new();
            for parent_ref in parent_refs {
                if let Some(port) = parent_ref.port {
                    ports.push(port as u16);
                } else if let Some(section_name) = &parent_ref.section_name {
                    let gw_ns = parent_ref
                        .namespace
                        .as_deref()
                        .or(tls.metadata.namespace.as_deref())
                        .unwrap_or("default");
                    if let Some(gateway) = lookup_gateway(gw_ns, &parent_ref.name) {
                        if let Some(listeners) = &gateway.spec.listeners {
                            if let Some(listener) = listeners.iter().find(|l| l.name == *section_name) {
                                ports.push(listener.port as u16);
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

    fn on_delete(&self, tls: &EdgionTls, ctx: &HandlerContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionTls,
            tls.metadata.namespace.clone(),
            tls.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);
        tracing::debug!(
            edgion_tls = %resource_ref.key(),
            "Cleared secret references on EdgionTls delete"
        );
    }

    fn update_status(&self, tls: &mut EdgionTls, _ctx: &HandlerContext, _validation_errors: &[String]) {
        let generation = tls.metadata.generation;

        // Check Secret resolution: if Secret is not found, set ResolvedRefs=False
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
            vec![ResolvedRefsError::BackendNotFound {
                namespace: secret_ns.map(|s| s.as_str()).unwrap_or("default").to_string(),
                name: tls.spec.secret_ref.name.clone(),
            }]
        };

        // Initialize status if not present
        let status = tls.status.get_or_insert_with(|| EdgionTlsStatus { parents: vec![] });

        // Update status for each parent ref
        if let Some(parent_refs) = &tls.spec.parent_refs {
            for parent_ref in parent_refs {
                let parent_status = status.parents.iter_mut().find(|ps| {
                    ps.parent_ref.name == parent_ref.name && ps.parent_ref.namespace == parent_ref.namespace
                });

                let update_conditions = |conditions: &mut Vec<_>| {
                    update_condition(conditions, accepted_condition(generation));
                    update_condition(conditions, programmed_condition(generation));
                    update_condition(conditions, ready_condition(generation));
                    if resolved_refs_errors.is_empty() {
                        update_condition(
                            conditions,
                            condition_true(
                                condition_types::RESOLVED_REFS,
                                condition_reasons::RESOLVED_REFS,
                                "Secret resolved",
                                generation,
                            ),
                        );
                    } else {
                        update_condition(
                            conditions,
                            condition_false(
                                condition_types::RESOLVED_REFS,
                                condition_reasons::BACKEND_NOT_FOUND,
                                resolved_refs_errors[0].message(),
                                generation,
                            ),
                        );
                    }
                };

                if let Some(ps) = parent_status {
                    update_conditions(&mut ps.conditions);
                } else {
                    let mut conditions = Vec::new();
                    update_conditions(&mut conditions);

                    status.parents.push(RouteParentStatus {
                        parent_ref: parent_ref.clone(),
                        controller_name: self.controller_name.clone(),
                        conditions,
                    });
                }
            }
        }
    }
}
