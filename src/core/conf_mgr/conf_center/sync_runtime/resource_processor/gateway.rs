//! Gateway Processor
//!
//! Handles Gateway resources with:
//! - Filter by gateway_class_name
//! - TLS Secret reference resolution
//! - SecretRefManager registration

use super::{find_secret, format_secret_key, ProcessContext, ProcessResult, ResourceProcessor};
use crate::core::conf_sync::conf_server_old::{ConfigServer, ResourceRef};
use crate::core::conf_sync::traits::{CacheEventDispatch, ResourceChange};
use crate::types::prelude_resources::Gateway;
use crate::types::ResourceKind;

/// Gateway processor
///
/// Features:
/// - filter: Filter by gateway_class_name (optional, None means no filter)
/// - parse: Parse TLS certificateRefs -> fill tls.secrets
/// - parse: Register Secret references to SecretRefManager
/// - on_delete: Clear SecretRefManager references
pub struct GatewayProcessor {
    /// If Some, only process Gateways with matching gatewayClassName
    /// If None, process all Gateways (used by FileSystem mode)
    gateway_class_name: Option<String>,
}

impl GatewayProcessor {
    /// Create a new GatewayProcessor
    ///
    /// - `gateway_class_name`: If Some, filter Gateways by this class name (K8s mode).
    ///   If None, process all Gateways (FileSystem mode).
    pub fn new(gateway_class_name: Option<String>) -> Self {
        Self { gateway_class_name }
    }
}

impl ResourceProcessor<Gateway> for GatewayProcessor {
    fn kind(&self) -> &'static str {
        "Gateway"
    }

    fn filter(&self, g: &Gateway) -> bool {
        match &self.gateway_class_name {
            Some(class_name) => g.spec.gateway_class_name == *class_name,
            None => true, // No filter, process all Gateways
        }
    }

    fn parse(&self, mut g: Gateway, ctx: &ProcessContext) -> ProcessResult<Gateway> {
        let resource_ref = ResourceRef::new(
            ResourceKind::Gateway,
            g.metadata.namespace.clone(),
            g.metadata.name.clone().unwrap_or_default(),
        );

        // Clear old references first (for update scenario)
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);

        // Process all Listeners and resolve TLS certificates
        if let Some(ref mut listeners) = g.spec.listeners {
            let secret_list = ctx.list_secrets();

            for listener in listeners.iter_mut() {
                let tls_config = match &mut listener.tls {
                    Some(tls) => tls,
                    None => continue,
                };

                if let Some(cert_refs) = &tls_config.certificate_refs {
                    if cert_refs.is_empty() {
                        continue;
                    }

                    let mut resolved_secrets = Vec::new();

                    for cert_ref in cert_refs {
                        let secret_ns = cert_ref.namespace.as_ref().or(g.metadata.namespace.as_ref());
                        let secret_key = format_secret_key(secret_ns, &cert_ref.name);

                        // Register to SecretRefManager (even if Secret doesn't exist yet)
                        ctx.secret_ref_manager()
                            .add_ref(secret_key.clone(), resource_ref.clone());

                        // Try to resolve Secret
                        if let Some(secret) = find_secret(&secret_list, secret_ns, &cert_ref.name) {
                            resolved_secrets.push(secret.clone());
                            tracing::debug!(
                                gateway = %resource_ref.key(),
                                listener = %listener.name,
                                secret_key = %secret_key,
                                "Secret resolved and filled into Gateway TLS config"
                            );
                        } else {
                            tracing::warn!(
                                gateway = %resource_ref.key(),
                                listener = %listener.name,
                                secret_key = %secret_key,
                                "Secret not found, Gateway TLS will be updated when Secret is added"
                            );
                        }
                    }

                    if !resolved_secrets.is_empty() {
                        tls_config.secrets = Some(resolved_secrets);
                    }
                }
            }
        }

        ProcessResult::Continue(g)
    }

    fn on_delete(&self, g: &Gateway, ctx: &ProcessContext) {
        let resource_ref = ResourceRef::new(
            ResourceKind::Gateway,
            g.metadata.namespace.clone(),
            g.metadata.name.clone().unwrap_or_default(),
        );
        ctx.secret_ref_manager().clear_resource_refs(&resource_ref);
        tracing::debug!(
            gateway = %resource_ref.key(),
            "Cleared secret references on Gateway delete"
        );
    }

    fn save(&self, cs: &ConfigServer, g: Gateway) {
        cs.gateways.apply_change(ResourceChange::EventUpdate, g);
    }

    fn remove(&self, cs: &ConfigServer, key: &str) {
        if let Some(obj) = cs.gateways.get_by_key(key) {
            cs.gateways.apply_change(ResourceChange::EventDelete, obj);
        }
    }

    fn get(&self, cs: &ConfigServer, key: &str) -> Option<Gateway> {
        cs.gateways.get_by_key(key)
    }
}
