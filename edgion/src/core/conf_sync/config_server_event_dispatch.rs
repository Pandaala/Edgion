use crate::core::conf_sync::traits::ResourceChange;
use crate::core::conf_sync::{
    ConfigServer, EventDispatch, EventDispatcher, ServerCache, Versionable,
};
use crate::types::{EdgionGatewayConfig, EdgionTls, Gateway, GatewayClass, HTTPRoute, ResourceKind};
use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::Resource;

impl ConfigServer {
    fn execute_change_on_cache<T>(
        change: ResourceChange,
        cache: &ServerCache<T>,
        resource: T,
        _resource_version: Option<u64>,
    ) where
        T: Clone + Send + Sync + 'static + Versionable + Resource,
    {
        cache.apply_change(change, resource);
    }
}

impl EventDispatcher for ConfigServer {
    fn apply_resource_change(
        &self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        resource_version: Option<u64>,
    ) {

        let resource_type = resource_type.or_else(|| ResourceKind::from_content(&data));
        
        if resource_type.is_none() {
            tracing::warn!(
                component = "config_server",
                event = "unknown_resource_type",
                data_preview = %data.chars().take(500).collect::<String>(),
                "Failed to determine resource type from content"
            );
            return;
        }
        
        let resource_type = resource_type.unwrap();

        // Skip base conf resources - they should be handled by apply_base_conf
        match resource_type {
            ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway => {
                tracing::debug!(
                    component = "config_server",
                    event = "skip_base_conf_in_apply_resource_change",
                    resource_type = ?resource_type,
                    "Base conf resources should be handled by apply_base_conf, skipping apply_resource_change"
                );
                return;
            }
            _ => {}
        }

        match resource_type {
            // Base conf resources are handled by apply_base_conf, not here
            ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway => {
                // This should never be reached due to the early return above,
                // but included for match exhaustiveness
                unreachable!("Base conf resources should have been handled earlier")
            }
            ResourceKind::HTTPRoute => {
                if let Ok(resource) = serde_json::from_str::<HTTPRoute>(&data) {
                    tracing::info!(
                        component = "config_server",
                        kind = "HTTPRoute",
                        "Applying HTTPRoute resource change"
                    );
                    Self::execute_change_on_cache::<HTTPRoute>(
                        change,
                        &self.routes,
                        resource,
                        resource_version,
                    );
                }
            }
            ResourceKind::Service => {
                if let Ok(resource) = serde_json::from_str::<Service>(&data) {
                    tracing::info!(
                        component = "config_server",
                        kind = "Service",
                        "Applying Service resource change"
                    );
                    Self::execute_change_on_cache::<Service>(
                        change,
                        &self.services,
                        resource,
                        resource_version,
                    );
                }
            }
            ResourceKind::EndpointSlice => {
                if let Ok(resource) = serde_json::from_str::<EndpointSlice>(&data) {
                    tracing::info!(
                        component = "config_server",
                        kind = "EndpointSlice",
                        "Applying EndpointSlice resource change"
                    );
                    Self::execute_change_on_cache::<EndpointSlice>(
                        change,
                        &self.endpoint_slices,
                        resource,
                        resource_version,
                    );
                }
            }
            ResourceKind::EdgionTls => {
                if let Ok(resource) = serde_json::from_str::<EdgionTls>(&data) {
                    tracing::info!(
                        component = "config_server",
                        kind = "EdgionTls",
                        "Applying EdgionTls resource change"
                    );
                    Self::execute_change_on_cache::<EdgionTls>(
                        change,
                        &self.edgion_tls,
                        resource,
                        resource_version,
                    );
                }
            }
            ResourceKind::Secret => {
                if let Ok(resource) = serde_json::from_str::<Secret>(&data) {
                    tracing::info!(
                        component = "config_server",
                        kind = "Secret",
                        "Applying Secret resource change"
                    );
                    Self::execute_change_on_cache::<Secret>(
                        change,
                        &self.secrets,
                        resource,
                        resource_version,
                    );
                }
            }
        }
    }

    fn set_ready(&self) {
        use crate::core::conf_sync::EventDispatch;

        // Base conf resources (GatewayClass, EdgionGatewayConfig, Gateway) don't have caches
        // They are stored in base_conf and don't need set_ready

        self.routes.set_ready();
        self.services.set_ready();
        self.endpoint_slices.set_ready();
        self.edgion_tls.set_ready();
        self.secrets.set_ready();
    }

    fn apply_base_conf(
        &self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        _resource_version: Option<u64>,
    ) {
        let resource_type = resource_type.or_else(|| ResourceKind::from_content(&data));
        
        if resource_type.is_none() {
            tracing::warn!(
                component = "config_server",
                event = "unknown_resource_type",
                data_preview = %data.chars().take(500).collect::<String>(),
                "Failed to determine resource type from content in apply_base_conf"
            );
            return;
        }
        
        let resource_type = resource_type.unwrap();

        // Only process base conf resources
        match resource_type {
            ResourceKind::GatewayClass => {
                if let Ok(resource) = serde_json::from_str::<GatewayClass>(&data) {
                    tracing::info!(
                        component = "config_server",
                        kind = "GatewayClass",
                        event = "apply_base_conf",
                        gateway_class_name = ?resource.metadata.name,
                        change = ?change,
                        "Applying GatewayClass to base_conf"
                    );
                    match change {
                        ResourceChange::InitAdd | ResourceChange::EventAdd | ResourceChange::EventUpdate => {
                            let mut base_conf = self.base_conf.write().unwrap();
                            base_conf.set_gateway_class(resource);
                        }
                        ResourceChange::EventDelete => {
                            let mut base_conf = self.base_conf.write().unwrap();
                            base_conf.clear_gateway_class();
                        }
                    }
                }
            }
            ResourceKind::EdgionGatewayConfig => {
                if let Ok(resource) = serde_json::from_str::<EdgionGatewayConfig>(&data) {
                    tracing::info!(
                        component = "config_server",
                        kind = "EdgionGatewayConfig",
                        event = "apply_base_conf",
                        edgion_gateway_config_name = ?resource.metadata.name,
                        change = ?change,
                        "Applying EdgionGatewayConfig to base_conf"
                    );
                    match change {
                        ResourceChange::InitAdd | ResourceChange::EventAdd | ResourceChange::EventUpdate => {
                            let mut base_conf = self.base_conf.write().unwrap();
                            base_conf.set_edgion_gateway_config(resource);
                        }
                        ResourceChange::EventDelete => {
                            let mut base_conf = self.base_conf.write().unwrap();
                            base_conf.clear_edgion_gateway_config();
                        }
                    }
                }
            }
            ResourceKind::Gateway => {
                if let Ok(resource) = serde_json::from_str::<Gateway>(&data) {
                    tracing::info!(
                        component = "config_server",
                        kind = "Gateway",
                        event = "apply_base_conf",
                        gateway_name = ?resource.metadata.name,
                        gateway_namespace = ?resource.metadata.namespace,
                        change = ?change,
                        "Applying Gateway to base_conf"
                    );
                    match change {
                        ResourceChange::InitAdd | ResourceChange::EventAdd | ResourceChange::EventUpdate => {
                            let mut base_conf = self.base_conf.write().unwrap();
                            base_conf.add_gateway(resource);
                        }
                        ResourceChange::EventDelete => {
                            let mut base_conf = self.base_conf.write().unwrap();
                            base_conf.remove_gateway(
                                resource.metadata.namespace.as_ref(),
                                resource.metadata.name.as_ref()
                            );
                        }
                    }
                }
            }
            _ => {
                tracing::warn!(
                    component = "config_server",
                    event = "invalid_resource_type_for_base_conf",
                    resource_type = ?resource_type,
                    "apply_base_conf called with non-base-conf resource type"
                );
            }
        }
    }
}
