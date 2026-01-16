use crate::core::conf_sync::conf_server::ConfigServer;
use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::{CacheEventDispatch, ResourceMeta, ServerCache};
use crate::types::prelude_resources::*;
use k8s_openapi::api::core::v1::{Endpoints, Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::Resource;

/// Helper function to execute change on cache
fn execute_change_on_cache<T>(change: ResourceChange, cache: &ServerCache<T>, resource: T)
where
    T: Clone + Send + Sync + 'static + ResourceMeta + Resource,
{
    cache.apply_change(change, resource);
}

impl ConfigServer {
    /// Check if a Gateway exists in the gateway cache
    fn has_gateway(&self, namespace: Option<&String>, name: Option<&String>) -> bool {
        if let Some(name_str) = name {
            let gateways = self.gateways.list_owned();
            gateways.data.iter().any(|gw| {
                let gw_name_matches = gw.metadata.name.as_ref() == Some(name_str);
                let gw_namespace_matches = match (namespace, &gw.metadata.namespace) {
                    (Some(ns), Some(gw_ns)) => ns == gw_ns,
                    (None, None) => true,
                    _ => false,
                };
                gw_name_matches && gw_namespace_matches
            })
        } else {
            false
        }
    }

    /// Apply HTTPRoute change with gateway validation
    pub fn apply_http_route_change(&self, change: ResourceChange, resource: HTTPRoute) {
        // Check if HTTPRoute references a gateway that exists
        let gateway_exists = if let Some(parent_refs) = &resource.spec.parent_refs {
            if let Some(first_ref) = parent_refs.first() {
                let gateway_namespace = first_ref.namespace.as_ref().or(resource.metadata.namespace.as_ref());
                let gateway_name = Some(&first_ref.name);

                self.has_gateway(gateway_namespace, gateway_name)
            } else {
                false
            }
        } else {
            false
        };

        if !gateway_exists {
            let (gateway_info, message) = if let Some(parent_refs) = &resource.spec.parent_refs {
                if let Some(first_ref) = parent_refs.first() {
                    let info = format!(
                        "namespace={:?}, name={}",
                        first_ref.namespace.as_ref().or(resource.metadata.namespace.as_ref()),
                        first_ref.name
                    );
                    (info, "HTTPRoute references a Gateway that does not exist, skipping")
                } else {
                    (
                        "no parent_refs".to_string(),
                        "HTTPRoute has empty parent_refs, skipping",
                    )
                }
            } else {
                ("no parent_refs".to_string(), "HTTPRoute has no parent_refs, skipping")
            };

            tracing::warn!(
                component = "config_server",
                change = ?change,
                kind = "HTTPRoute",
                route_name = ?resource.metadata.name,
                route_namespace = ?resource.metadata.namespace,
                gateway = gateway_info,
                "{}",
                message
            );
            return;
        }

        tracing::info!(
            component = "config_server",
            change = ?change,
            kind = "HTTPRoute",
            "Applying HTTPRoute resource change"
        );
        execute_change_on_cache(change, &self.routes, resource);
    }

    /// Apply GRPCRoute change with gateway validation
    pub fn apply_grpc_route_change(&self, change: ResourceChange, resource: GRPCRoute) {
        let gateway_exists = if let Some(parent_refs) = &resource.spec.parent_refs {
            if let Some(first_ref) = parent_refs.first() {
                let gateway_namespace = first_ref.namespace.as_ref().or(resource.metadata.namespace.as_ref());
                let gateway_name = Some(&first_ref.name);

                self.has_gateway(gateway_namespace, gateway_name)
            } else {
                false
            }
        } else {
            false
        };

        if !gateway_exists {
            let (gateway_info, message) = if let Some(parent_refs) = &resource.spec.parent_refs {
                if let Some(first_ref) = parent_refs.first() {
                    let info = format!(
                        "namespace={:?}, name={}",
                        first_ref.namespace.as_ref().or(resource.metadata.namespace.as_ref()),
                        first_ref.name
                    );
                    (info, "GRPCRoute references a Gateway that does not exist, skipping")
                } else {
                    (
                        "no parent_refs".to_string(),
                        "GRPCRoute has empty parent_refs, skipping",
                    )
                }
            } else {
                ("no parent_refs".to_string(), "GRPCRoute has no parent_refs, skipping")
            };

            tracing::warn!(
                component = "config_server",
                change = ?change,
                kind = "GRPCRoute",
                route_name = ?resource.metadata.name,
                route_namespace = ?resource.metadata.namespace,
                gateway = gateway_info,
                "{}",
                message
            );
            return;
        }

        tracing::info!(
            component = "config_server",
            change = ?change,
            kind = "GRPCRoute",
            "Applying GRPCRoute resource change"
        );
        execute_change_on_cache(change, &self.grpc_routes, resource);
    }

    /// Apply TCPRoute change with gateway validation
    pub fn apply_tcp_route_change(&self, change: ResourceChange, resource: TCPRoute) {
        let gateway_exists = if let Some(parent_refs) = &resource.spec.parent_refs {
            if let Some(first_ref) = parent_refs.first() {
                let gateway_namespace = first_ref.namespace.as_ref().or(resource.metadata.namespace.as_ref());
                let gateway_name = Some(&first_ref.name);

                self.has_gateway(gateway_namespace, gateway_name)
            } else {
                false
            }
        } else {
            false
        };

        if !gateway_exists {
            let (gateway_info, message) = if let Some(parent_refs) = &resource.spec.parent_refs {
                if let Some(first_ref) = parent_refs.first() {
                    let info = format!(
                        "namespace={:?}, name={}",
                        first_ref.namespace.as_ref().or(resource.metadata.namespace.as_ref()),
                        first_ref.name
                    );
                    (info, "TCPRoute references a Gateway that does not exist, skipping")
                } else {
                    ("no parent_refs".to_string(), "TCPRoute has empty parent_refs, skipping")
                }
            } else {
                ("no parent_refs".to_string(), "TCPRoute has no parent_refs, skipping")
            };

            tracing::warn!(
                component = "config_server",
                change = ?change,
                kind = "TCPRoute",
                route_name = ?resource.metadata.name,
                route_namespace = ?resource.metadata.namespace,
                gateway = gateway_info,
                "{}",
                message
            );
            return;
        }

        tracing::info!(
            component = "config_server",
            change = ?change,
            kind = "TCPRoute",
            "Applying TCPRoute resource change"
        );
        execute_change_on_cache(change, &self.tcp_routes, resource);
    }

    /// Apply UDPRoute change with gateway validation
    pub fn apply_udp_route_change(&self, change: ResourceChange, resource: UDPRoute) {
        let gateway_exists = if let Some(parent_refs) = &resource.spec.parent_refs {
            if let Some(first_ref) = parent_refs.first() {
                let gateway_namespace = first_ref.namespace.as_ref().or(resource.metadata.namespace.as_ref());
                let gateway_name = Some(&first_ref.name);

                self.has_gateway(gateway_namespace, gateway_name)
            } else {
                false
            }
        } else {
            false
        };

        if !gateway_exists {
            let (gateway_info, message) = if let Some(parent_refs) = &resource.spec.parent_refs {
                if let Some(first_ref) = parent_refs.first() {
                    let info = format!(
                        "namespace={:?}, name={}",
                        first_ref.namespace.as_ref().or(resource.metadata.namespace.as_ref()),
                        first_ref.name
                    );
                    (info, "UDPRoute references a Gateway that does not exist, skipping")
                } else {
                    ("no parent_refs".to_string(), "UDPRoute has empty parent_refs, skipping")
                }
            } else {
                ("no parent_refs".to_string(), "UDPRoute has no parent_refs, skipping")
            };

            tracing::warn!(
                component = "config_server",
                change = ?change,
                kind = "UDPRoute",
                route_name = ?resource.metadata.name,
                route_namespace = ?resource.metadata.namespace,
                gateway = gateway_info,
                "{}",
                message
            );
            return;
        }

        tracing::info!(
            component = "config_server",
            change = ?change,
            kind = "UDPRoute",
            "Applying UDPRoute resource change"
        );
        execute_change_on_cache(change, &self.udp_routes, resource);
    }

    /// Apply TLSRoute change with gateway validation
    pub fn apply_tls_route_change(&self, change: ResourceChange, resource: TLSRoute) {
        let gateway_exists = if let Some(parent_refs) = &resource.spec.parent_refs {
            if let Some(first_ref) = parent_refs.first() {
                let gateway_namespace = first_ref.namespace.as_ref().or(resource.metadata.namespace.as_ref());
                let gateway_name = Some(&first_ref.name);

                self.has_gateway(gateway_namespace, gateway_name)
            } else {
                false
            }
        } else {
            false
        };

        if !gateway_exists {
            tracing::warn!(
                component = "config_server",
                change = ?change,
                kind = "TLSRoute",
                route_name = ?resource.metadata.name,
                route_namespace = ?resource.metadata.namespace,
                "TLSRoute references a Gateway that does not exist, skipping"
            );
            return;
        }

        tracing::info!(
            component = "config_server",
            change = ?change,
            kind = "TLSRoute",
            "Applying TLSRoute resource change"
        );
        execute_change_on_cache(change, &self.tls_routes, resource);
    }

    /// Apply EdgionTls change with secret reference handling
    ///
    /// Note: Gateway existence check is now handled by callers via `resource_check::check_edgion_tls`.
    /// This method focuses on Secret resolution and SecretRefManager maintenance.
    pub fn apply_edgion_tls_change(&self, change: ResourceChange, mut resource: EdgionTls) {
        // Handle Secret reference
        use super::secret_ref::ResourceRef;
        use crate::types::ResourceKind as RK;

        let resource_ref = ResourceRef::new(
            RK::EdgionTls,
            resource.metadata.namespace.clone(),
            resource.metadata.name.clone().unwrap_or_default(),
        );

        // Build secret key from secret_ref
        let secret_namespace = resource
            .spec
            .secret_ref
            .namespace
            .as_ref()
            .or(resource.metadata.namespace.as_ref());
        let secret_key = if let Some(ns) = secret_namespace {
            format!("{}/{}", ns, resource.spec.secret_ref.name)
        } else {
            resource.spec.secret_ref.name.clone()
        };

        // Register reference relationship
        match change {
            ResourceChange::InitAdd | ResourceChange::EventAdd | ResourceChange::EventUpdate => {
                self.secret_ref_manager
                    .add_ref(secret_key.clone(), resource_ref.clone());

                // Try to resolve Secret immediately from cache
                let secret_list = self.secrets.list_owned();
                let secret_data = secret_list.data.iter().find(|s| {
                    let s_namespace = s.metadata.namespace.as_deref();
                    let s_name = s.metadata.name.as_deref().unwrap_or("");
                    s_namespace == secret_namespace.map(|s| s.as_str()) && s_name == resource.spec.secret_ref.name
                });

                if let Some(secret) = secret_data {
                    resource.spec.secret = Some(secret.clone());
                    tracing::debug!(
                        edgion_tls = %resource_ref.key(),
                        secret_key = %secret_key,
                        "Secret resolved and filled into EdgionTls"
                    );
                } else {
                    tracing::warn!(
                        edgion_tls = %resource_ref.key(),
                        secret_key = %secret_key,
                        "Secret not found, EdgionTls will be sent without Secret data"
                    );
                }

                // Also load CA Secret if mTLS is configured
                if let Some(ref mut client_auth) = resource.spec.client_auth {
                    if let Some(ref ca_secret_ref) = client_auth.ca_secret_ref {
                        let ca_secret_namespace = ca_secret_ref
                            .namespace
                            .as_ref()
                            .or(resource.metadata.namespace.as_ref());

                        let ca_secret_data = secret_list.data.iter().find(|s| {
                            let s_namespace = s.metadata.namespace.as_deref();
                            let s_name = s.metadata.name.as_deref().unwrap_or("");
                            s_namespace == ca_secret_namespace.map(|s| s.as_str()) && s_name == ca_secret_ref.name
                        });

                        if let Some(ca_secret) = ca_secret_data {
                            client_auth.ca_secret = Some(ca_secret.clone());
                            tracing::debug!(
                                edgion_tls = %resource_ref.key(),
                                ca_secret_name = %ca_secret_ref.name,
                                "CA Secret resolved and filled into EdgionTls.client_auth"
                            );
                        } else {
                            tracing::warn!(
                                edgion_tls = %resource_ref.key(),
                                ca_secret_name = %ca_secret_ref.name,
                                "CA Secret not found, mTLS will not work"
                            );
                        }

                        // Register CA Secret reference
                        let ca_secret_key = if let Some(ns) = ca_secret_namespace {
                            format!("{}/{}", ns, ca_secret_ref.name)
                        } else {
                            ca_secret_ref.name.clone()
                        };
                        self.secret_ref_manager.add_ref(ca_secret_key, resource_ref.clone());
                    }
                }
            }
            ResourceChange::EventDelete => {
                self.secret_ref_manager.clear_resource_refs(&resource_ref);
            }
            ResourceChange::InitStart | ResourceChange::InitDone => {
                // Signal events: handled by cache directly
            }
        }

        tracing::info!(
            component = "config_server",
            kind = "EdgionTls",
            "Applying EdgionTls resource change"
        );
        execute_change_on_cache(change, &self.edgion_tls, resource);
    }

    /// Apply Gateway change with TLS secret reference handling
    ///
    /// Similar to EdgionTls, Gateway can reference Secrets via `certificateRefs` in TLS config.
    /// This function:
    /// 1. Registers Secret references with SecretRefManager
    /// 2. Resolves and fills in Secret data to `tls.secrets` field
    /// 3. Enables cascading updates when referenced Secrets change
    pub fn apply_gateway_change(&self, change: ResourceChange, mut resource: Gateway) {
        use super::secret_ref::ResourceRef;
        use crate::types::ResourceKind as RK;

        // Create Gateway resource reference
        let resource_ref = ResourceRef::new(
            RK::Gateway,
            resource.metadata.namespace.clone(),
            resource.metadata.name.clone().unwrap_or_default(),
        );

        // Handle delete: clear all Secret references
        if matches!(change, ResourceChange::EventDelete) {
            self.secret_ref_manager.clear_resource_refs(&resource_ref);
            tracing::info!(
                component = "config_server",
                kind = "Gateway",
                gateway = %resource_ref.key(),
                "Applying Gateway delete, cleared secret references"
            );
            execute_change_on_cache(change, &self.gateways, resource);
            return;
        }

        // Clear old references for update scenario
        if matches!(change, ResourceChange::EventUpdate) {
            self.secret_ref_manager.clear_resource_refs(&resource_ref);
        }

        // Process all Listeners and resolve TLS certificates
        let mut has_tls_config = false;
        if let Some(ref mut listeners) = resource.spec.listeners {
            let secret_list = self.secrets.list_owned();

            for listener in listeners.iter_mut() {
                let tls_config = match &mut listener.tls {
                    Some(tls) => tls,
                    None => continue,
                };

                if let Some(cert_refs) = &tls_config.certificate_refs {
                    if cert_refs.is_empty() {
                        continue;
                    }

                    has_tls_config = true;
                    let mut resolved_secrets = Vec::new();

                    for cert_ref in cert_refs {
                        let secret_ns = cert_ref
                            .namespace
                            .as_ref()
                            .or(resource.metadata.namespace.as_ref());

                        // Build secret key and register reference
                        let secret_key = if let Some(ns) = secret_ns {
                            format!("{}/{}", ns, cert_ref.name)
                        } else {
                            cert_ref.name.clone()
                        };

                        // Register to SecretRefManager (even if Secret doesn't exist yet)
                        self.secret_ref_manager
                            .add_ref(secret_key.clone(), resource_ref.clone());

                        // Try to find and resolve Secret
                        if let Some(secret) = secret_list.data.iter().find(|s| {
                            s.metadata.namespace.as_deref() == secret_ns.map(|s| s.as_str())
                                && s.metadata.name.as_deref() == Some(cert_ref.name.as_str())
                        }) {
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

        tracing::info!(
            component = "config_server",
            kind = "Gateway",
            gateway = %resource_ref.key(),
            has_tls = has_tls_config,
            "Applying Gateway resource change"
        );
        execute_change_on_cache(change, &self.gateways, resource);
    }

    /// Apply Service change
    pub fn apply_service_change(&self, change: ResourceChange, resource: Service) {
        tracing::info!(
            component = "config_server",
            kind = "Service",
            "Applying Service resource change"
        );
        execute_change_on_cache(change, &self.services, resource);
    }

    /// Apply EndpointSlice change
    pub fn apply_endpoint_slice_change(&self, change: ResourceChange, resource: EndpointSlice) {
        tracing::info!(
            component = "config_server",
            kind = "EndpointSlice",
            "Applying EndpointSlice resource change"
        );
        execute_change_on_cache(change, &self.endpoint_slices, resource);
    }

    /// Apply Endpoints change
    pub fn apply_endpoint_change(&self, change: ResourceChange, resource: Endpoints) {
        tracing::info!(
            component = "config_server",
            kind = "Endpoints",
            "Applying Endpoints resource change"
        );
        execute_change_on_cache(change, &self.endpoints, resource);
    }

    /// Apply EdgionPlugins change
    pub fn apply_edgion_plugins_change(&self, change: ResourceChange, resource: EdgionPlugins) {
        tracing::info!(
            component = "config_server",
            kind = "EdgionPlugins",
            "Applying EdgionPlugins resource change"
        );
        execute_change_on_cache(change, &self.edgion_plugins, resource);
    }

    /// Apply PluginMetaData change
    pub fn apply_plugin_metadata_change(&self, change: ResourceChange, resource: PluginMetaData) {
        tracing::info!(
            component = "config_server",
            kind = "PluginMetaData",
            metadata_name = ?resource.metadata.name,
            data_type = ?resource.data_type(),
            "Applying PluginMetaData resource change"
        );
        execute_change_on_cache(change, &self.plugin_metadata, resource);
    }

    /// Apply LinkSys change
    pub fn apply_link_sys_change(&self, change: ResourceChange, resource: LinkSys) {
        tracing::info!(
            component = "config_server",
            change = ?change,
            kind = "LinkSys",
            "Applying LinkSys resource change"
        );
        execute_change_on_cache(change, &self.link_sys, resource);
    }

    /// Apply Secret change with cascading updates
    pub fn apply_secret_change(&self, change: ResourceChange, resource: Secret) {
        tracing::info!(
            component = "config_server",
            kind = "Secret",
            "Applying Secret resource change"
        );

        // Build secret key
        let secret_namespace = resource.metadata.namespace.as_ref();
        let secret_name = resource.metadata.name.as_deref().unwrap_or("");
        let secret_key = if let Some(ns) = secret_namespace {
            format!("{}/{}", ns, secret_name)
        } else {
            secret_name.to_string()
        };

        // Apply the Secret change first
        execute_change_on_cache(change, &self.secrets, resource.clone());

        // Also update global SecretStore for TLS callback access
        use super::secret_store::update_secrets;
        use std::collections::{HashMap, HashSet};
        match change {
            ResourceChange::InitAdd | ResourceChange::EventAdd => {
                let mut add = HashMap::new();
                add.insert(secret_key.clone(), resource.clone());
                update_secrets(add, HashMap::new(), &HashSet::new());
            }
            ResourceChange::EventUpdate => {
                let mut update = HashMap::new();
                update.insert(secret_key.clone(), resource.clone());
                update_secrets(HashMap::new(), update, &HashSet::new());
            }
            ResourceChange::EventDelete => {
                let mut remove = HashSet::new();
                remove.insert(secret_key.clone());
                update_secrets(HashMap::new(), HashMap::new(), &remove);
            }
            ResourceChange::InitStart | ResourceChange::InitDone => {
                // Signal events: no secret store updates needed
            }
        }

        // Handle resource references when Secret is added or updated
        match change {
            ResourceChange::InitAdd | ResourceChange::EventAdd | ResourceChange::EventUpdate => {
                // Get all resources that reference this Secret
                let refs = self.secret_ref_manager.get_refs(&secret_key);

                if !refs.is_empty() {
                    tracing::info!(
                        secret_key = %secret_key,
                        ref_count = refs.len(),
                        "Secret updated, triggering cascading updates for referencing resources"
                    );
                }

                use crate::types::ResourceKind as RK;
                for resource_ref in refs {
                    match resource_ref.kind {
                        RK::EdgionTls => {
                            // Reload EdgionTls from cache
                            let edgion_tls_list = self.edgion_tls.list_owned();
                            let _secret_list = self.secrets.list_owned();

                            if let Some(mut edgion_tls) = edgion_tls_list.data.into_iter().find(|tls| {
                                let tls_namespace = tls.metadata.namespace.as_deref();
                                let tls_name = tls.metadata.name.as_deref().unwrap_or("");
                                tls_namespace == resource_ref.namespace.as_deref() && tls_name == resource_ref.name
                            }) {
                                // Check if this Secret is the server cert or CA cert
                                let is_server_cert = edgion_tls.spec.secret_ref.name == secret_name;
                                let is_ca_cert = edgion_tls
                                    .spec
                                    .client_auth
                                    .as_ref()
                                    .and_then(|ca| ca.ca_secret_ref.as_ref())
                                    .map(|ca_ref| ca_ref.name == secret_name)
                                    .unwrap_or(false);

                                if is_server_cert {
                                    // Fill in the server Secret
                                    edgion_tls.spec.secret = Some(resource.clone());
                                    tracing::debug!(
                                        edgion_tls = %resource_ref.key(),
                                        secret_key = %secret_key,
                                        "Updating EdgionTls with resolved server Secret"
                                    );
                                }

                                if is_ca_cert {
                                    // Fill in the CA Secret
                                    if let Some(ref mut client_auth) = edgion_tls.spec.client_auth {
                                        client_auth.ca_secret = Some(resource.clone());
                                        tracing::info!(
                                            edgion_tls = %resource_ref.key(),
                                            secret_key = %secret_key,
                                            "Updating EdgionTls with resolved CA Secret (cascading update)"
                                        );
                                    }
                                }

                                // Update resource version for cascading update
                                if is_server_cert || is_ca_cert {
                                    use crate::core::utils;
                                    let new_version = utils::next_resource_version();
                                    edgion_tls.metadata.resource_version = Some(new_version.to_string());
                                    tracing::debug!(
                                        edgion_tls = %resource_ref.key(),
                                        new_version = new_version,
                                        "Updated resource version for cascading update"
                                    );
                                }

                                // Trigger update event
                                execute_change_on_cache(ResourceChange::EventUpdate, &self.edgion_tls, edgion_tls);
                            }
                        }
                        RK::Gateway => {
                            // Reload Gateway from cache and update TLS secrets
                            let gateway_list = self.gateways.list_owned();

                            if let Some(mut gateway) = gateway_list.data.into_iter().find(|gw| {
                                gw.metadata.namespace.as_deref() == resource_ref.namespace.as_deref()
                                    && gw.metadata.name.as_deref() == Some(resource_ref.name.as_str())
                            }) {
                                let mut updated = false;

                                // Iterate all Listeners and update matching TLS secrets
                                if let Some(ref mut listeners) = gateway.spec.listeners {
                                    for listener in listeners.iter_mut() {
                                        if let Some(ref mut tls_config) = listener.tls {
                                            if let Some(cert_refs) = &tls_config.certificate_refs {
                                                // Check if any certificateRef matches this Secret
                                                let matching_refs: Vec<_> = cert_refs
                                                    .iter()
                                                    .filter(|cert_ref| {
                                                        let ref_ns = cert_ref
                                                            .namespace
                                                            .as_ref()
                                                            .or(gateway.metadata.namespace.as_ref());
                                                        ref_ns.map(|s| s.as_str()) == secret_namespace.map(|s| s.as_str())
                                                            && cert_ref.name == secret_name
                                                    })
                                                    .collect();

                                                if !matching_refs.is_empty() {
                                                    // Rebuild secrets list for this listener
                                                    let mut resolved_secrets = Vec::new();
                                                    let all_secrets = self.secrets.list_owned();

                                                    for cert_ref in cert_refs {
                                                        let ref_ns = cert_ref
                                                            .namespace
                                                            .as_ref()
                                                            .or(gateway.metadata.namespace.as_ref());

                                                        // If this is the updated secret, use it
                                                        if ref_ns.map(|s| s.as_str()) == secret_namespace.map(|s| s.as_str())
                                                            && cert_ref.name == secret_name
                                                        {
                                                            resolved_secrets.push(resource.clone());
                                                        } else {
                                                            // Find other secrets from cache
                                                            if let Some(other_secret) = all_secrets.data.iter().find(|s| {
                                                                s.metadata.namespace.as_deref()
                                                                    == ref_ns.map(|s| s.as_str())
                                                                    && s.metadata.name.as_deref()
                                                                        == Some(cert_ref.name.as_str())
                                                            }) {
                                                                resolved_secrets.push(other_secret.clone());
                                                            }
                                                        }
                                                    }

                                                    tls_config.secrets = Some(resolved_secrets);
                                                    updated = true;

                                                    tracing::debug!(
                                                        gateway = %resource_ref.key(),
                                                        listener = %listener.name,
                                                        secret_key = %secret_key,
                                                        "Updated Gateway TLS config with resolved Secret"
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }

                                if updated {
                                    // Update resource version for cascading update
                                    use crate::core::utils;
                                    let new_version = utils::next_resource_version();
                                    gateway.metadata.resource_version = Some(new_version.to_string());

                                    tracing::info!(
                                        gateway = %resource_ref.key(),
                                        secret_key = %secret_key,
                                        new_version = new_version,
                                        "Updating Gateway with resolved TLS Secret (cascading update)"
                                    );

                                    // Trigger update event
                                    execute_change_on_cache(ResourceChange::EventUpdate, &self.gateways, gateway);
                                }
                            }
                        }
                        _ => {
                            tracing::warn!(
                                resource = %resource_ref.key(),
                                "Unexpected resource kind referencing Secret, skipping"
                            );
                        }
                    }
                }
            }
            ResourceChange::EventDelete => {
                tracing::debug!(
                    secret_key = %secret_key,
                    "Secret deleted, referencing resources will have empty Secret field"
                );
            }
            ResourceChange::InitStart | ResourceChange::InitDone => {
                // Signal events: no cascading updates needed
            }
        }
    }
}
