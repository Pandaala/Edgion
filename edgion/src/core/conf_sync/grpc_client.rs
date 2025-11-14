use crate::core::conf_sync::config_client::ConfigClient;
use crate::core::conf_sync::proto::{
    config_sync_client::ConfigSyncClient as ConfigSyncClientService, ListRequest, ListResponse,
    ResourceKind as ProtoResourceKind, WatchRequest, WatchResponse,
};
use crate::core::conf_sync::traits::{EventDispatcher, ResourceChange};
use crate::types::ResourceKind;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tonic::transport::Channel;
use uuid::Uuid;

/// gRPC client for ConfigSync service
pub struct ConfigSyncClient {
    client: ConfigSyncClientService<Channel>,
    config_client: Arc<Mutex<ConfigClient>>,
    client_id: String,
    client_name: String,
}

impl ConfigSyncClient {
    /// Create a new ConfigSync client connected to the given address
    pub async fn connect(
        addr: String,
        gateway_class_key: String,
    ) -> Result<Self, tonic::transport::Error> {
        let client = ConfigSyncClientService::connect(addr).await?;
        let config_client = Arc::new(Mutex::new(ConfigClient::new(gateway_class_key)));
        let client_id = Uuid::new_v4().to_string();
        let client_name = "config-sync-client".to_string();
        Ok(Self {
            client,
            config_client,
            client_id,
            client_name,
        })
    }

    /// Create a new ConfigSync client with custom timeout
    pub async fn connect_with_timeout(
        addr: String,
        gateway_class_key: String,
        timeout: Duration,
    ) -> Result<Self, tonic::transport::Error> {
        let endpoint = tonic::transport::Endpoint::from_shared(addr)?
            .timeout(timeout)
            .connect_timeout(timeout);
        let channel = endpoint.connect().await?;
        let client = ConfigSyncClientService::new(channel);
        let config_client = Arc::new(Mutex::new(ConfigClient::new(gateway_class_key)));
        let client_id = Uuid::new_v4().to_string();
        let client_name = "config-sync-client".to_string();
        Ok(Self {
            client,
            config_client,
            client_id,
            client_name,
        })
    }

    /// Get a reference to the ConfigHub
    pub fn get_config_client(&self) -> Arc<Mutex<ConfigClient>> {
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

        let hub = self.config_client.lock().await;
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
        let hub = self.config_client.lock().await;
        let from_version = match kind {
            ResourceKind::GatewayClass => hub.list_gateway_classes().resource_version,
            ResourceKind::EdgionGatewayConfig => hub.list_edgion_gateway_config().resource_version,
            ResourceKind::Gateway => hub.list_gateways().resource_version,
            ResourceKind::HTTPRoute => hub.list_routes().resource_version,
            ResourceKind::Service => hub.list_services().resource_version,
            ResourceKind::EndpointSlice => hub.list_endpoint_slices().resource_version,
            ResourceKind::EdgionTls => hub.list_edgion_tls().resource_version,
            ResourceKind::Secret => hub.list_secrets().resource_version,
        };
        drop(hub);

        let mut receiver = self
            .watch(
                key.clone(),
                kind,
                self.client_id.clone(),
                self.client_name.clone(),
                from_version,
            )
            .await?;

        let hub_clone = self.config_client.clone();
        let kind_clone = kind;

        tokio::spawn(async move {
            while let Some(watch_response) = receiver.recv().await {
                // Parse the events from the watch response
                // Watch response contains a JSON array of events with type, data, and resource_version
                let events: Vec<serde_json::Value> =
                    match serde_json::from_str(&watch_response.data) {
                        Ok(events) => events,
                        Err(e) => {
                            eprintln!("Failed to parse watch response events: {}", e);
                            continue;
                        }
                    };

                let hub = hub_clone.lock().await;
                for event in events {
                    if let Some(event_type) = event.get("type").and_then(|v| v.as_str()) {
                        let data_str = match serde_json::to_string(
                            &event.get("data").unwrap_or(&serde_json::Value::Null),
                        ) {
                            Ok(s) => s,
                            Err(e) => {
                                eprintln!("Failed to serialize event data: {}", e);
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
        });

        Ok(())
    }

    /// Sync all resource types from server
    pub async fn sync_all(&mut self) -> Result<(), tonic::Status> {
        let hub = self.config_client.lock().await;
        let key = hub.get_gateway_class_key().clone();
        drop(hub);

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
        let hub = self.config_client.lock().await;
        let key = hub.get_gateway_class_key().clone();
        drop(hub);

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
        let request = tonic::Request::new(ListRequest {
            key,
            kind: resource_kind_to_proto(kind) as i32,
        });

        let response = self.client.list(request).await?;
        Ok(response.into_inner())
    }

    /// Watch for changes to resources of a specific kind
    pub async fn watch(
        &mut self,
        key: String,
        kind: ResourceKind,
        client_id: String,
        client_name: String,
        from_version: u64,
    ) -> Result<mpsc::Receiver<WatchResponse>, tonic::Status> {
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

        let mut stream = self.client.watch(request).await?.into_inner();

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
