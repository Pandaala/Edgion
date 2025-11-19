use crate::core::conf_sync::config_client::ConfigClient;
use crate::core::conf_sync::proto::{
    config_sync_client::ConfigSyncClient as ConfigSyncClientService, 
    GetBaseConfRequest, ListRequest, ListResponse,
    ResourceKind as ProtoResourceKind, WatchRequest, WatchResponse,
};
use crate::core::conf_sync::traits::{EventDispatcher, ResourceChange};
use crate::types::ResourceKind;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tonic::transport::Channel;
use tracing;
use uuid::Uuid;

/// gRPC client for ConfigSync service
pub struct ConfigSyncClient {
    grpc_server_addr: String,
    config_client: Arc<ConfigClient>,
    client_id: String,
    client_name: String,
    grpc_server_connect_timeout: Duration,
    conf_client_handle: Option<ConfigSyncClientService<Channel>>,
}

impl ConfigSyncClient {
    /// Create a new ConfigSync client without connecting to server
    pub fn new(
        grpc_server_addr: &str,
        gateway_class_key: String,
        client_name: String,
        timeout: Duration,
    ) -> Self {
        let config_client = Arc::new(ConfigClient::new(gateway_class_key));
        let client_id = Uuid::new_v4().to_string();
        Self {
            grpc_server_addr: grpc_server_addr.to_string(),
            config_client,
            client_id,
            client_name,
            grpc_server_connect_timeout: timeout,
            conf_client_handle: None,
        }
    }

    /// Connect to the gRPC server
    pub async fn connect(&mut self) -> Result<(), tonic::transport::Error> {
        let endpoint = tonic::transport::Endpoint::from_shared(self.grpc_server_addr.clone())?
            .timeout(self.grpc_server_connect_timeout)
            .connect_timeout(self.grpc_server_connect_timeout);
        let channel = endpoint.connect().await?;
        let client = ConfigSyncClientService::new(channel);
        self.conf_client_handle = Some(client);
        Ok(())
    }

    /// Check if the client is connected
    pub fn is_connected(&self) -> bool {
        self.conf_client_handle.is_some()
    }

    /// Get a reference to the ConfigHub
    pub fn get_config_client(&self) -> Arc<ConfigClient> {
        self.config_client.clone()
    }

    /// Fetch and initialize base configuration from server
    async fn fetch_and_init_base_conf(
        &mut self,
        gateway_class_key: &str,
    ) -> Result<(), tonic::Status> {
        let client = self
            .conf_client_handle
            .as_mut()
            .ok_or_else(|| tonic::Status::failed_precondition("Client not connected"))?;

        let request = tonic::Request::new(GetBaseConfRequest {
            gateway_class: gateway_class_key.to_string(),
        });

        let response = client.get_base_conf(request).await?;
        let base_conf_response = response.into_inner();

        tracing::info!(
            gateway_class_bytes = base_conf_response.gateway_class.len(),
            edgion_gateway_config_bytes = base_conf_response.edgion_gateway_config.len(),
            gateways_bytes = base_conf_response.gateways.len(),
            "Init base_conf"
        );

        // Parse JSON data and build GatewayClassBaseConf in ConfigClient
        let base_conf = ConfigClient::parse_base_conf_from_json(
            base_conf_response.gateway_class,
            base_conf_response.edgion_gateway_config,
            base_conf_response.gateways,
        );

        // Initialize base_conf in ConfigClient
        self.config_client.init_base_conf(base_conf);

        tracing::info!("Base configuration initialized");
        Ok(())
    }

    /// Initialize base configuration (GatewayClass, EdgionGatewayConfig, Gateway)
    /// and sync all other resources (HTTPRoute, Service, etc.)
    pub async fn init(&mut self) -> Result<(), tonic::Status> {
        let key = self.config_client.get_gateway_class_key().clone();

        // Step 1: Get base configuration
        self.fetch_and_init_base_conf(&key).await?;

        // Step 2: List and sync all other resources
        let resource_kinds = vec![
            ResourceKind::HTTPRoute,
            ResourceKind::Service,
            ResourceKind::EndpointSlice,
            ResourceKind::EdgionTls,
            ResourceKind::Secret,
        ];

        for kind in resource_kinds {
            if let Err(e) = self.sync_resource(key.clone(), kind).await {
                tracing::error!(kind = ?kind, error = %e, "Failed to init sync");
            }
        }

        tracing::info!("All resources initialized");
        Ok(())
    }

    /// Sync all resources of a specific kind from server
    pub async fn sync_resource(
        &mut self,
        key: String,
        kind: ResourceKind,
    ) -> Result<(), tonic::Status> {
        let list_response = self.list(key.clone(), kind).await?;

        tracing::info!(
            kind = ?kind,
            bytes = list_response.data.len(),
            version = list_response.resource_version,
            "Syncing resource"
        );

        // Parse the JSON data - list returns an array of resources
        let resources: Vec<serde_json::Value> =
            serde_json::from_str(&list_response.data).map_err(|e| {
                tracing::error!(
                    kind = ?kind,
                    error = %e,
                    data_preview = %&list_response.data[..list_response.data.len().min(200)],
                    "Failed to parse list response"
                );
                tonic::Status::internal(format!("Failed to parse list response: {}", e))
            })?;

        tracing::info!(
            kind = ?kind,
            count = resources.len(),
            "Parsed resources"
        );

        let hub = &self.config_client;
        for (idx, resource) in resources.iter().enumerate() {
            // Each resource in the list should be added/updated
            let data_str = serde_json::to_string(&resource).map_err(|e| {
                tracing::error!(
                    kind = ?kind,
                    index = idx,
                    error = %e,
                    "Failed to serialize resource"
                );
                tonic::Status::internal(format!("Failed to serialize resource: {}", e))
            })?;

            hub.apply_resource_change(
                ResourceChange::InitAdd,
                Some(kind),
                data_str,
                Some(list_response.resource_version),
            );
        }

        tracing::info!(kind = ?kind, "Finished syncing");
        Ok(())
    }

    /// Start watching a specific resource kind and automatically sync to ConfigHub
    pub async fn start_watch_sync(
        &mut self,
        key: String,
        kind: ResourceKind,
    ) -> Result<(), tonic::Status> {
        let hub_clone = self.config_client.clone();
        let kind_clone = kind;
        let client_id = self.client_id.clone();
        let client_name = self.client_name.clone();
        let grpc_server_addr = self.grpc_server_addr.clone();
        let grpc_server_connect_timeout = self.grpc_server_connect_timeout;

        tokio::spawn(async move {
            let mut from_version = {
                let hub = &hub_clone;
                match kind_clone {
                    // Base conf resources should not be watched
                    ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway => {
                        tracing::warn!(kind = ?kind_clone, "Attempted to watch base_conf resource, which should not be watched");
                        return;
                    }
                    ResourceKind::HTTPRoute => hub.list_routes().resource_version,
                    ResourceKind::Service => hub.list_services().resource_version,
                    ResourceKind::EndpointSlice => hub.list_endpoint_slices().resource_version,
                    ResourceKind::EdgionTls => hub.list_edgion_tls().resource_version,
                    ResourceKind::Secret => hub.list_secrets().resource_version,
                }
            };

            loop {
                // Connect and create watch stream
                let mut client = match Self::create_client(&grpc_server_addr, grpc_server_connect_timeout).await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!(
                            kind = ?kind_clone,
                            client_id = %client_id,
                            error = %e,
                            "Failed to create client for watch"
                        );
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                };

                let request = tonic::Request::new(WatchRequest {
                    key: key.clone(),
                    kind: resource_kind_to_proto(kind_clone) as i32,
                    client_id: client_id.clone(),
                    client_name: client_name.clone(),
                    from_version,
                });

                let mut stream = match client.watch(request).await {
                    Ok(response) => response.into_inner(),
                    Err(e) => {
                        tracing::error!(
                            kind = ?kind_clone,
                            client_id = %client_id,
                            error = %e,
                            "Failed to start watch"
                        );
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                };

                tracing::info!(
                    kind = ?kind_clone,
                    client_id = %client_id,
                    from_version = from_version,
                    "Watch started"
                );

                // Process stream messages
                loop {
                    match stream.message().await {
                        Ok(Some(watch_response)) => {
                            // Update from_version for next reconnect
                            from_version = watch_response.resource_version;

                            // Parse the events from the watch response
                            let events: Vec<serde_json::Value> = match serde_json::from_str(&watch_response.data) {
                                Ok(events) => events,
                                Err(e) => {
                                    tracing::error!(error = %e, "Failed to parse watch response events");
                                    continue;
                                }
                            };

                            let hub = &hub_clone;
                            for event in events {
                                if let Some(event_type) = event.get("type").and_then(|v| v.as_str()) {
                                    let data_str = match serde_json::to_string(
                                        &event.get("data").unwrap_or(&serde_json::Value::Null),
                                    ) {
                                        Ok(s) => s,
                                        Err(e) => {
                                            tracing::error!(error = %e, "Failed to serialize event data");
                                            continue;
                                        }
                                    };
                                    let resource_version = event
                                        .get("resource_version")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(watch_response.resource_version);

                                    match event_type {
                                        "add" => hub.apply_resource_change(
                                            ResourceChange::EventAdd,
                                            Some(kind_clone),
                                            data_str,
                                            Some(resource_version),
                                        ),
                                        "update" => hub.apply_resource_change(
                                            ResourceChange::EventUpdate,
                                            Some(kind_clone),
                                            data_str,
                                            Some(resource_version),
                                        ),
                                        "delete" => hub.apply_resource_change(
                                            ResourceChange::EventDelete,
                                            Some(kind_clone),
                                            data_str,
                                            Some(resource_version),
                                        ),
                                        _ => {}
                                    }
                                }
                            }
                        }
                        Ok(None) => {
                            // Stream ended
                            tracing::info!(
                                kind = ?kind_clone,
                                client_id = %client_id,
                                "Watch stream ended, reconnecting"
                            );
                            break;
                        }
                        Err(e) => {
                            // Stream error
                            tracing::error!(
                                kind = ?kind_clone,
                                client_id = %client_id,
                                error = %e,
                                "Watch stream error, reconnecting"
                            );
                            break;
                        }
                    }
                }

                // Wait before reconnecting
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });

        Ok(())
    }

    /// Helper method to create a new client
    async fn create_client(
        addr: &str,
        timeout: Duration,
    ) -> Result<ConfigSyncClientService<Channel>, tonic::transport::Error> {
        let endpoint = tonic::transport::Endpoint::from_shared(addr.to_string())?
            .timeout(timeout)
            .connect_timeout(timeout);
        let channel = endpoint.connect().await?;
        Ok(ConfigSyncClientService::new(channel))
    }

    /// Sync all resource types from server (excluding base_conf resources)
    pub async fn sync_all(&mut self) -> Result<(), tonic::Status> {
        let hub = &self.config_client;
        let key = hub.get_gateway_class_key().clone();

        // Only sync non-base_conf resources
        let resource_kinds = vec![
            ResourceKind::HTTPRoute,
            ResourceKind::Service,
            ResourceKind::EndpointSlice,
            ResourceKind::EdgionTls,
            ResourceKind::Secret,
        ];

        for kind in resource_kinds {
            if let Err(e) = self.sync_resource(key.clone(), kind).await {
                tracing::error!(kind = ?kind, error = %e, "Failed to sync");
            }
        }

        Ok(())
    }

    /// Start watching all resource types and automatically sync to ConfigHub (excluding base_conf resources)
    pub async fn start_watch_all(&mut self) -> Result<(), tonic::Status> {
        let hub = &self.config_client;
        let key = hub.get_gateway_class_key().clone();

        // Only watch non-base_conf resources
        let resource_kinds = vec![
            ResourceKind::HTTPRoute,
            ResourceKind::Service,
            ResourceKind::EndpointSlice,
            ResourceKind::EdgionTls,
            ResourceKind::Secret,
        ];

        for kind in resource_kinds {
            if let Err(e) = self.start_watch_sync(key.clone(), kind).await {
                tracing::error!(kind = ?kind, error = %e, "Failed to start watch");
            }
        }

        Ok(())
    }

    /// List resources of a specific kind
    pub async fn list(
        &mut self,
        key: String,
        kind: ResourceKind,
    ) -> Result<ListResponse, tonic::Status> {
        let client = self
            .conf_client_handle
            .as_mut()
            .ok_or_else(|| tonic::Status::failed_precondition("Client not connected"))?;

        let request = tonic::Request::new(ListRequest {
            key,
            kind: resource_kind_to_proto(kind) as i32,
        });

        let response = client.list(request).await?;
        Ok(response.into_inner())
    }

    /// Watch for changes to resources of a specific kind (internal use only)
    #[allow(dead_code)]
    async fn watch(
        &mut self,
        key: String,
        kind: ResourceKind,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Result<mpsc::Receiver<WatchResponse>, tonic::Status> {
        let client = self
            .conf_client_handle
            .as_mut()
            .ok_or_else(|| tonic::Status::failed_precondition("Client not connected"))?;

        tracing::info!(
            key = %key,
            kind = ?kind,
            client_id = %client_id,
            client_name = %client_name,
            "Start watch"
        );
        let request = tonic::Request::new(WatchRequest {
            key,
            kind: resource_kind_to_proto(kind) as i32,
            client_id,
            client_name,
            from_version,
        });

        let mut stream = client.watch(request).await?.into_inner();

        // Convert stream to mpsc::Receiver
        let (tx, rx) = mpsc::channel(100);

        tokio::spawn(async move {
            while let Some(result) = stream.message().await.transpose() {
                match result {
                    Ok(response) => {
                        if tx.send(response).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Error receiving watch response");
                        break;
                    }
                }
            }
        });

        Ok(rx)
    }
}

/// Convert our ResourceKind to proto ResourceKind
fn resource_kind_to_proto(kind: ResourceKind) -> ProtoResourceKind {
    match kind {
        ResourceKind::GatewayClass => ProtoResourceKind::GatewayClass,
        ResourceKind::EdgionGatewayConfig => ProtoResourceKind::GatewayClassSpec,
        ResourceKind::Gateway => ProtoResourceKind::Gateway,
        ResourceKind::HTTPRoute => ProtoResourceKind::HttpRoute,
        ResourceKind::Service => ProtoResourceKind::Service,
        ResourceKind::EndpointSlice => ProtoResourceKind::EndpointSlice,
        ResourceKind::EdgionTls => ProtoResourceKind::EdgionTls,
        ResourceKind::Secret => ProtoResourceKind::Secret,
    }
}
