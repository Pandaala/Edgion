use crate::core::conf_sync::config_client::ConfigClient;
use crate::core::conf_sync::proto::{
    config_sync_client::ConfigSyncClient as ConfigSyncClientService, ListRequest, ListResponse,
    ResourceKind as ProtoResourceKind, WatchRequest, WatchResponse,
};
use crate::core::conf_sync::traits::{EventDispatcher, ResourceChange};
use crate::types::ResourceKind;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tonic::transport::Channel;
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
        grpc_server_addr: String,
        gateway_class_key: String,
        client_name: String,
        timeout: Duration,
    ) -> Self {
        let config_client = Arc::new(ConfigClient::new(gateway_class_key));
        let client_id = Uuid::new_v4().to_string();
        Self {
            grpc_server_addr,
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

    /// Sync all resources of a specific kind from server
    pub async fn sync_resource(
        &mut self,
        key: String,
        kind: ResourceKind,
    ) -> Result<(), tonic::Status> {
        let list_response = self.list(key.clone(), kind).await?;

        println!(
            "[CLIENT] Syncing {:?}: received {} bytes, version {}",
            kind,
            list_response.data.len(),
            list_response.resource_version
        );

        // Parse the JSON data - list returns an array of resources
        let resources: Vec<serde_json::Value> =
            serde_json::from_str(&list_response.data).map_err(|e| {
                eprintln!(
                    "[CLIENT] Failed to parse list response for {:?}: {} (data: {})",
                    kind,
                    e,
                    &list_response.data[..list_response.data.len().min(200)]
                );
                tonic::Status::internal(format!("Failed to parse list response: {}", e))
            })?;

        println!(
            "[CLIENT] Parsed {} resources for {:?}",
            resources.len(),
            kind
        );

        let hub = &self.config_client;
        for (idx, resource) in resources.iter().enumerate() {
            // Each resource in the list should be added/updated
            let data_str = serde_json::to_string(&resource).map_err(|e| {
                eprintln!(
                    "[CLIENT] Failed to serialize resource {} for {:?}: {}",
                    idx, kind, e
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

        println!("[CLIENT] Finished syncing {:?}", kind);
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
                    ResourceKind::GatewayClass => hub.list_gateway_classes().resource_version,
                    ResourceKind::EdgionGatewayConfig => hub.list_edgion_gateway_config().resource_version,
                    ResourceKind::Gateway => hub.list_gateways().resource_version,
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
                        eprintln!(
                            "[CLIENT] Failed to create client for watch (kind={:?}, client_id={}): {}",
                            kind_clone, client_id, e
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
                        eprintln!(
                            "[CLIENT] Failed to start watch (kind={:?}, client_id={}): {}",
                            kind_clone, client_id, e
                        );
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                };

                println!(
                    "[CLIENT] Watch started for {:?} (client_id={}, from_version={})",
                    kind_clone, client_id, from_version
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
                                    eprintln!("[CLIENT] Failed to parse watch response events: {}", e);
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
                                            eprintln!("[CLIENT] Failed to serialize event data: {}", e);
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
                            eprintln!(
                                "[CLIENT] Watch stream ended for {:?} (client_id={}), reconnecting...",
                                kind_clone, client_id
                            );
                            break;
                        }
                        Err(e) => {
                            // Stream error
                            eprintln!(
                                "[CLIENT] Watch stream error for {:?} (client_id={}): {}, reconnecting...",
                                kind_clone, client_id, e
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

    /// Sync all resource types from server
    pub async fn sync_all(&mut self) -> Result<(), tonic::Status> {
        let hub = &self.config_client;
        let key = hub.get_gateway_class_key().clone();

        let resource_kinds = vec![
            ResourceKind::GatewayClass,
            ResourceKind::EdgionGatewayConfig,
            ResourceKind::Gateway,
            ResourceKind::HTTPRoute,
            ResourceKind::Service,
            ResourceKind::EndpointSlice,
            ResourceKind::EdgionTls,
            ResourceKind::Secret,
        ];

        for kind in resource_kinds {
            if let Err(e) = self.sync_resource(key.clone(), kind).await {
                eprintln!("Failed to sync {:?}: {}", kind, e);
            }
        }

        Ok(())
    }

    /// Start watching all resource types and automatically sync to ConfigHub
    pub async fn start_watch_all(&mut self) -> Result<(), tonic::Status> {
        let hub = &self.config_client;
        let key = hub.get_gateway_class_key().clone();

        let resource_kinds = vec![
            ResourceKind::GatewayClass,
            ResourceKind::EdgionGatewayConfig,
            ResourceKind::Gateway,
            ResourceKind::HTTPRoute,
            ResourceKind::Service,
            ResourceKind::EndpointSlice,
            ResourceKind::EdgionTls,
            ResourceKind::Secret,
        ];

        for kind in resource_kinds {
            if let Err(e) = self.start_watch_sync(key.clone(), kind).await {
                eprintln!("Failed to start watch for {:?}: {}", kind, e);
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

        println!(
            "start watch for {:?} kind={:?} client_id={:?} client_name={:?}",
            key, kind, client_id, client_name
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
                        eprintln!("Error receiving watch response: {}", e);
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
