//! Kubernetes Controller
//!
//! Watches Kubernetes resources and keeps the KubernetesStore cache up to date

use std::sync::Arc;
use anyhow::Result;
use futures::{StreamExt, TryStreamExt};
use kube::{Api, Client, ResourceExt};
use kube::runtime::watcher;

use crate::core::conf_sync::{ConfigServer, CacheEventDispatch};
use crate::core::conf_sync::traits::ResourceChange;
use crate::types::prelude_resources::*;
use super::KubernetesStore;

use k8s_openapi::api::core::v1::{Service, Secret, Endpoints};
use k8s_openapi::api::discovery::v1::EndpointSlice;

/// Kubernetes Controller that watches resources and updates ConfigServer
pub struct KubernetesController {
    client: Client,
    config_server: Arc<ConfigServer>,
    store: Arc<KubernetesStore>,
    #[allow(dead_code)]
    gateway_class_name: String,
}

impl KubernetesController {
    pub async fn new(
        config_server: Arc<ConfigServer>,
        store: Arc<KubernetesStore>,
        gateway_class_name: String,
    ) -> Result<Self> {
        let client = Client::try_default().await?;
        Ok(Self {
            client,
            config_server,
            store,
            gateway_class_name,
        })
    }
    
    /// Run all watchers concurrently
    pub async fn run(&self) -> Result<()> {
        tracing::info!(
            component = "k8s_controller",
            event = "controller_start",
            "Starting Kubernetes controller"
        );
        
        // Run all watchers concurrently
        // Note: If any watcher fails, all will be cancelled
        tokio::try_join!(
            // Base conf resources
            self.watch_gateway_classes(),
            self.watch_gateways(),
            self.watch_edgion_gateway_configs(),
            // Route resources
            self.watch_http_routes(),
            self.watch_grpc_routes(),
            self.watch_tcp_routes(),
            self.watch_udp_routes(),
            self.watch_tls_routes(),
            // Backend resources
            self.watch_services(),
            self.watch_endpoints(),
            self.watch_endpoint_slices(),
            // Other resources
            self.watch_secrets(),
            self.watch_reference_grants(),
            self.watch_edgion_plugins(),
            self.watch_edgion_stream_plugins(),
            self.watch_edgion_tls(),
            self.watch_backend_tls_policies(),
            self.watch_plugin_metadata(),
            self.watch_link_sys(),
        )?;
        
        Ok(())
    }
    
    // Helper function to handle watch events for most resources
    async fn handle_event<T>(
        &self,
        event: watcher::Event<T>,
        kind: &str,
        handler: impl Fn(&ConfigServer, ResourceChange, T),
    ) -> Result<()>
    where
        T: kube::Resource + Clone + serde::Serialize,
        <T as kube::Resource>::DynamicType: Default,
    {
        match event {
            watcher::Event::Apply(resource) => {
                let yaml = serde_yaml::to_string(&resource)?;
                self.store.apply_resource(
                    kind.to_string(),
                    kube::ResourceExt::namespace(&resource),
                    kube::ResourceExt::name_any(&resource),
                    yaml,
                ).await;
                handler(&self.config_server, ResourceChange::EventAdd, resource);
            }
            watcher::Event::Delete(resource) => {
                self.store.remove_resource(
                    kind,
                    kube::ResourceExt::namespace(&resource).as_deref(),
                    &kube::ResourceExt::name_any(&resource),
                ).await;
                handler(&self.config_server, ResourceChange::EventDelete, resource);
            }
            watcher::Event::Init => {}
            watcher::Event::InitApply(resource) => {
                let yaml = serde_yaml::to_string(&resource)?;
                self.store.apply_resource(
                    kind.to_string(),
                    kube::ResourceExt::namespace(&resource),
                    kube::ResourceExt::name_any(&resource),
                    yaml,
                ).await;
                handler(&self.config_server, ResourceChange::InitAdd, resource);
            }
            watcher::Event::InitDone => {
                tracing::info!("{} watcher initial sync complete", kind);
            }
        }
        Ok(())
    }
    
    async fn watch_http_routes(&self) -> Result<()> {
        let api: Api<HTTPRoute> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "HTTPRoute", |server, change, resource| {
                server.routes.apply_change(change, resource);
            }).await?;
        }
        Ok(())
    }
    
    async fn watch_grpc_routes(&self) -> Result<()> {
        let api: Api<GRPCRoute> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "GRPCRoute", |server, change, resource| {
                server.grpc_routes.apply_change(change, resource);
            }).await?;
        }
        Ok(())
    }
    
    async fn watch_tcp_routes(&self) -> Result<()> {
        let api: Api<TCPRoute> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "TCPRoute", |server, change, resource| {
                server.tcp_routes.apply_change(change, resource);
            }).await?;
        }
        Ok(())
    }
    
    async fn watch_udp_routes(&self) -> Result<()> {
        let api: Api<UDPRoute> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "UDPRoute", |server, change, resource| {
                server.udp_routes.apply_change(change, resource);
            }).await?;
        }
        Ok(())
    }
    
    async fn watch_tls_routes(&self) -> Result<()> {
        let api: Api<TLSRoute> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "TLSRoute", |server, change, resource| {
                server.tls_routes.apply_change(change, resource);
            }).await?;
        }
        Ok(())
    }
    
    async fn watch_services(&self) -> Result<()> {
        let api: Api<Service> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "Service", |server, change, resource| {
                server.services.apply_change(change, resource);
            }).await?;
        }
        Ok(())
    }
    
    async fn watch_endpoints(&self) -> Result<()> {
        let api: Api<Endpoints> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "Endpoints", |server, change, resource| {
                server.endpoints.apply_change(change, resource);
            }).await?;
        }
        Ok(())
    }
    
    async fn watch_endpoint_slices(&self) -> Result<()> {
        let api: Api<EndpointSlice> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "EndpointSlice", |server, change, resource| {
                server.endpoint_slices.apply_change(change, resource);
            }).await?;
        }
        Ok(())
    }
    
    async fn watch_secrets(&self) -> Result<()> {
        let api: Api<Secret> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "Secret", |server, change, resource| {
                server.apply_secret_change(change, resource);
            }).await?;
        }
        Ok(())
    }
    
    async fn watch_reference_grants(&self) -> Result<()> {
        let api: Api<ReferenceGrant> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "ReferenceGrant", |server, change, resource| {
                server.reference_grants.apply_change(change, resource);
            }).await?;
        }
        Ok(())
    }
    
    async fn watch_edgion_plugins(&self) -> Result<()> {
        let api: Api<EdgionPlugins> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "EdgionPlugins", |server, change, resource| {
                server.edgion_plugins.apply_change(change, resource);
            }).await?;
        }
        Ok(())
    }
    
    async fn watch_edgion_stream_plugins(&self) -> Result<()> {
        let api: Api<EdgionStreamPlugins> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "EdgionStreamPlugins", |server, change, resource| {
                server.edgion_stream_plugins.apply_change(change, resource);
            }).await?;
        }
        Ok(())
    }
    
    async fn watch_edgion_tls(&self) -> Result<()> {
        let api: Api<EdgionTls> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "EdgionTls", |server, change, resource| {
                server.apply_edgion_tls_change(change, resource);
            }).await?;
        }
        Ok(())
    }
    
    async fn watch_backend_tls_policies(&self) -> Result<()> {
        let api: Api<BackendTLSPolicy> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "BackendTLSPolicy", |server, change, resource| {
                server.backend_tls_policies.apply_change(change, resource);
            }).await?;
        }
        Ok(())
    }
    
    async fn watch_plugin_metadata(&self) -> Result<()> {
        let api: Api<PluginMetaData> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "PluginMetaData", |server, change, resource| {
                server.plugin_metadata.apply_change(change, resource);
            }).await?;
        }
        Ok(())
    }
    
    async fn watch_link_sys(&self) -> Result<()> {
        let api: Api<LinkSys> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "LinkSys", |server, change, resource| {
                server.link_sys.apply_change(change, resource);
            }).await?;
        }
        Ok(())
    }
    
    // Base conf resources watchers
    
    async fn watch_gateway_classes(&self) -> Result<()> {
        let api: Api<GatewayClass> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "GatewayClass", |server, change, resource| {
                server.gateway_classes.apply_change(change, resource);
            }).await?;
        }
        Ok(())
    }
    
    async fn watch_gateways(&self) -> Result<()> {
        let api: Api<Gateway> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "Gateway", |server, change, resource| {
                server.gateways.apply_change(change, resource);
            }).await?;
        }
        Ok(())
    }
    
    async fn watch_edgion_gateway_configs(&self) -> Result<()> {
        let api: Api<EdgionGatewayConfig> = Api::all(self.client.clone());
        let watcher = watcher(api, Default::default());
        tokio::pin!(watcher);
        while let Some(event) = watcher.try_next().await? {
            self.handle_event(event, "EdgionGatewayConfig", |server, change, resource| {
                server.edgion_gateway_configs.apply_change(change, resource);
            }).await?;
        }
        Ok(())
    }
}
