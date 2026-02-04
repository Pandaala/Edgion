//! EdgionPlugins Handler
//!
//! Handles EdgionPlugins resources with Gateway API standard status management.
//! Also resolves Secret references for plugins like JwtAuth.
//!
//! Features:
//! - parse: Resolve Secret references and register to SecretRefManager for cascading updates
//! - on_delete: Clear SecretRefManager references

use crate::core::conf_mgr::sync_runtime::resource_processor::{
    condition_types, format_secret_key, get_secret, HandlerContext, ProcessResult, ProcessorHandler, ResourceRef,
};
use crate::types::prelude_resources::EdgionPlugins;
use crate::types::resources::edgion_plugins::plugin_configs::ResolvedJwtCredential;
use crate::types::resources::edgion_plugins::{EdgionPlugin, EdgionPluginsStatus};
use crate::types::ResourceKind;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use k8s_openapi::chrono::Utc;
use std::collections::HashMap;

/// EdgionPlugins handler
///
/// Features:
/// - parse: Resolve JWT Secret references and register to SecretRefManager
/// - on_delete: Clear SecretRefManager references
pub struct EdgionPluginsHandler;

impl EdgionPluginsHandler {
    pub fn new() -> Self {
        Self
    }

    /// Resolve JWT credentials from Secrets and register references to SecretRefManager
    fn resolve_jwt_credentials(ep: &mut EdgionPlugins, resource_ref: &ResourceRef, ctx: &HandlerContext) {
        let ep_ns = ep.metadata.namespace.as_deref().unwrap_or("default");

        // Process request plugins
        if let Some(ref mut plugins) = ep.spec.request_plugins {
            for entry in plugins.iter_mut() {
                if let EdgionPlugin::JwtAuth(ref mut config) = entry.plugin {
                    // Resolve single secret_ref
                    if let Some(ref secret_ref) = config.secret_ref {
                        let ns = secret_ref.namespace.as_ref().or(ep.metadata.namespace.as_ref());
                        let secret_key = format_secret_key(ns, &secret_ref.name);
                        let ns_str = ns.map(|s| s.as_str()).unwrap_or(ep_ns);

                        // Register reference relationship (critical for cascading updates)
                        // When this Secret arrives or updates, SecretHandler will trigger requeue
                        ctx.secret_ref_manager()
                            .add_ref(secret_key.clone(), resource_ref.clone());

                        if let Some(secret) = get_secret(Some(ns_str), &secret_ref.name) {
                            if let Some(data) = &secret.data {
                                let mut cred = ResolvedJwtCredential::default();
                                // Try "secret" key for HS*
                                if let Some(secret_bytes) = data.get("secret") {
                                    cred.secret = Some(STANDARD.encode(&secret_bytes.0));
                                }
                                // Try "publicKey" key for RS*/ES*
                                if let Some(pk_bytes) = data.get("publicKey") {
                                    cred.public_key = String::from_utf8(pk_bytes.0.clone()).ok();
                                }
                                if cred.secret.is_some() || cred.public_key.is_some() {
                                    config.resolved_credential = Some(cred);
                                    tracing::debug!(
                                        edgion_plugins = %resource_ref.key(),
                                        secret_key = %secret_key,
                                        "JwtAuth: Secret resolved and credential filled"
                                    );
                                }
                            }
                        } else {
                            // Secret not found yet - this is normal if Secret arrives after EdgionPlugins
                            // The SecretRefManager reference ensures we'll be requeued when Secret arrives
                            tracing::info!(
                                edgion_plugins = %resource_ref.key(),
                                secret_key = %secret_key,
                                "JwtAuth: Secret not found yet, will be reprocessed when Secret arrives"
                            );
                        }
                    }

                    // Resolve multiple secret_refs
                    if let Some(ref secret_refs) = config.secret_refs {
                        let mut resolved = HashMap::new();
                        for secret_ref in secret_refs {
                            let ns = secret_ref.namespace.as_ref().or(ep.metadata.namespace.as_ref());
                            let secret_key = format_secret_key(ns, &secret_ref.name);
                            let ns_str = ns.map(|s| s.as_str()).unwrap_or(ep_ns);

                            // Register reference for cascading updates
                            ctx.secret_ref_manager()
                                .add_ref(secret_key.clone(), resource_ref.clone());

                            if let Some(secret) = get_secret(Some(ns_str), &secret_ref.name) {
                                if let Some(data) = &secret.data {
                                    let mut cred = ResolvedJwtCredential::default();
                                    // Get "key" identifier
                                    if let Some(key_bytes) = data.get("key") {
                                        cred.key = String::from_utf8(key_bytes.0.clone()).ok();
                                    }
                                    // Get "secret" for HS*
                                    if let Some(secret_bytes) = data.get("secret") {
                                        cred.secret = Some(STANDARD.encode(&secret_bytes.0));
                                    }
                                    // Get "publicKey" for RS*/ES*
                                    if let Some(pk_bytes) = data.get("publicKey") {
                                        cred.public_key = String::from_utf8(pk_bytes.0.clone()).ok();
                                    }
                                    if let Some(ref key) = cred.key {
                                        resolved.insert(key.clone(), cred);
                                    }
                                }
                            }
                        }
                        if !resolved.is_empty() {
                            config.resolved_credentials = Some(resolved);
                        }
                    }
                }
            }
        }
    }
}

impl Default for EdgionPluginsHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessorHandler<EdgionPlugins> for EdgionPluginsHandler {
    fn preparse(&self, ep: &mut EdgionPlugins, _ctx: &HandlerContext) -> Vec<String> {
        // Build plugin runtime and collect validation errors
        ep.preparse();
        ep.get_preparse_errors().to_vec()
    }

    fn parse(&self, mut ep: EdgionPlugins, ctx: &HandlerContext) -> ProcessResult<EdgionPlugins> {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            ep.metadata.namespace.clone(),
            ep.metadata.name.clone().unwrap_or_default(),
        );

        // Clear old references first (for update scenario)
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);

        // Resolve JWT credentials from Secrets and register references
        Self::resolve_jwt_credentials(&mut ep, &resource_ref, ctx);

        // Note: preparse() is called by processor before parse(), so we don't call it here

        ProcessResult::Continue(ep)
    }

    fn on_delete(&self, ep: &EdgionPlugins, ctx: &HandlerContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::EdgionPlugins,
            ep.metadata.namespace.clone(),
            ep.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);
        tracing::debug!(
            edgion_plugins = %resource_ref.key(),
            "Cleared secret references on EdgionPlugins delete"
        );
    }

    fn update_status(&self, ep: &mut EdgionPlugins, _ctx: &HandlerContext, validation_errors: &[String]) {
        let generation = ep.metadata.generation;

        // Note: validation_errors already includes preparse errors (merged by processor)

        // Initialize status if not present
        let status = ep
            .status
            .get_or_insert_with(|| EdgionPluginsStatus { conditions: vec![] });

        // Set Accepted condition
        let accepted = if validation_errors.is_empty() {
            k8s_condition_true(condition_types::ACCEPTED, "Accepted", "Resource accepted", generation)
        } else {
            k8s_condition_false(
                condition_types::ACCEPTED,
                "Invalid",
                &validation_errors.join("; "),
                generation,
            )
        };
        update_k8s_condition(&mut status.conditions, accepted);

        // Set Ready condition (ready only if no errors)
        let ready = if validation_errors.is_empty() {
            k8s_condition_true(condition_types::READY, "Ready", "Resource is ready", generation)
        } else {
            k8s_condition_false(
                condition_types::READY,
                "ConfigurationError",
                "Resource has configuration errors",
                generation,
            )
        };
        update_k8s_condition(&mut status.conditions, ready);
    }
}

/// Create a k8s_openapi Condition with True status
fn k8s_condition_true(type_: &str, reason: &str, message: &str, observed_generation: Option<i64>) -> Condition {
    Condition {
        type_: type_.to_string(),
        status: "True".to_string(),
        reason: reason.to_string(),
        message: message.to_string(),
        last_transition_time: Time(Utc::now()),
        observed_generation,
    }
}

/// Create a k8s_openapi Condition with False status
fn k8s_condition_false(type_: &str, reason: &str, message: &str, observed_generation: Option<i64>) -> Condition {
    Condition {
        type_: type_.to_string(),
        status: "False".to_string(),
        reason: reason.to_string(),
        message: message.to_string(),
        last_transition_time: Time(Utc::now()),
        observed_generation,
    }
}

/// Update or insert a k8s_openapi Condition
fn update_k8s_condition(conditions: &mut Vec<Condition>, new_condition: Condition) {
    if let Some(existing) = conditions.iter_mut().find(|c| c.type_ == new_condition.type_) {
        let status_changed = existing.status != new_condition.status;
        existing.status = new_condition.status;
        existing.reason = new_condition.reason;
        existing.message = new_condition.message;
        existing.observed_generation = new_condition.observed_generation;
        if status_changed {
            existing.last_transition_time = new_condition.last_transition_time;
        }
    } else {
        conditions.push(new_condition);
    }
}
