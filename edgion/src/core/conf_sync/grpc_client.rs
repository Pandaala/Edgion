use crate::core::conf_sync::base_onf::GatewayClassBaseConf;
use crate::core::conf_sync::config_client::ConfigClient;
use crate::core::conf_sync::proto::{
    config_sync_client::ConfigSyncClient as ConfigSyncClientService, GetBaseConfRequest,
    ListRequest, ListResponse, WatchRequest, WatchResponse,
};
use crate::core::conf_sync::traits::{ConfigClientEventDispatcher, ResourceChange};
use crate::types::{
    EdgionGatewayConfig, EdgionTls, Gateway, GatewayClass, HTTPRoute, ResourceKind,
};
use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use serde::de::DeserializeOwned;
use serde::Serialize;
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
    conf_client_handle: ConfigSyncClientService<Channel>,
}

impl ConfigSyncClient {
    /// Create a new ConfigSync client and connect to server with retry
    /// Retries connection up to 3 times with 2 second intervals
    pub async fn new(
        grpc_server_addr: &str,
        gateway_class_key: String,
        client_name: String,
        timeout: Duration,
    ) -> Result<Self, tonic::transport::Error> {
        let config_client = Arc::new(ConfigClient::new(gateway_class_key));
        let client_id = Uuid::new_v4().to_string();

        // Try to connect with retry logic: 3 attempts, 2 seconds apart
        const MAX_RETRIES: u32 = 3;
        const RETRY_INTERVAL_SECS: u64 = 2;

        let mut last_error = None;
        for attempt in 1..=MAX_RETRIES {
            tracing::info!(
                attempt = attempt,
                max_retries = MAX_RETRIES,
                server_addr = grpc_server_addr,
                "Attempting to connect to gRPC server"
            );

            match Self::create_client_internal(grpc_server_addr, timeout).await {
                Ok(client) => {
                    tracing::info!(
                        server_addr = grpc_server_addr,
                        "Successfully connected to gRPC server"
                    );

                    return Ok(Self {
                        grpc_server_addr: grpc_server_addr.to_string(),
                        config_client,
                        client_id,
                        client_name,
                        grpc_server_connect_timeout: timeout,
                        conf_client_handle: client,
                    });
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < MAX_RETRIES {
                        tracing::warn!(
                            attempt = attempt,
                            max_retries = MAX_RETRIES,
                            error = %last_error.as_ref().unwrap(),
                            retry_in_secs = RETRY_INTERVAL_SECS,
                            "Failed to connect, will retry"
                        );
                        tokio::time::sleep(Duration::from_secs(RETRY_INTERVAL_SECS)).await;
                    }
                }
            }
        }

        let err = last_error.unwrap();
        tracing::error!(
            server_addr = grpc_server_addr,
            error = %err,
            "Failed to connect to gRPC server after {} attempts",
            MAX_RETRIES
        );
        Err(err)
    }

    /// Internal helper to create a client
    async fn create_client_internal(
        addr: &str,
        timeout: Duration,
    ) -> Result<ConfigSyncClientService<Channel>, tonic::transport::Error> {
        let endpoint = tonic::transport::Endpoint::from_shared(addr.to_string())?
            .timeout(timeout)
            .connect_timeout(timeout);
        let channel = endpoint.connect().await?;
        Ok(ConfigSyncClientService::new(channel))
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
        let request = tonic::Request::new(GetBaseConfRequest {
            gateway_class: gateway_class_key.to_string(),
        });

        let response = self.conf_client_handle.get_base_conf(request).await?;
        let base_conf_response = response.into_inner();

        tracing::info!(
            gateway_class_bytes = base_conf_response.gateway_class.len(),
            edgion_gateway_config_bytes = base_conf_response.edgion_gateway_config.len(),
            gateways_bytes = base_conf_response.gateways.len(),
            "Init base_conf"
        );

        let mut base_conf = GatewayClassBaseConf::new();

        // Parse and set GatewayClass
        if !base_conf_response.gateway_class.is_empty() {
            if let Ok(items) =
                serde_json::from_str::<Vec<GatewayClass>>(&base_conf_response.gateway_class)
            {
                if let Some(gc) = items.into_iter().next() {
                    println!("[ConfigClient] Parsed GatewayClass");
                    base_conf.set_gateway_class(gc);
                }
            }
        }

        // Parse and set EdgionGatewayConfig
        if !base_conf_response.edgion_gateway_config.is_empty() {
            if let Ok(items) = serde_json::from_str::<Vec<EdgionGatewayConfig>>(
                &base_conf_response.edgion_gateway_config,
            ) {
                if let Some(egc) = items.into_iter().next() {
                    println!("[ConfigClient] Parsed EdgionGatewayConfig");
                    base_conf.set_edgion_gateway_config(egc);
                }
            }
        }

        // Parse and add Gateways
        if !base_conf_response.gateways.is_empty() {
            if let Ok(gateways) = serde_json::from_str::<Vec<Gateway>>(&base_conf_response.gateways)
            {
                for gateway in gateways {
                    println!("[ConfigClient] Parsed Gateway");
                    base_conf.add_gateway(gateway);
                }
            }
        }

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

        let hub = &self.config_client;
        match kind {
            ResourceKind::Unspecified => {
                return Err(tonic::Status::invalid_argument(
                    "Cannot sync unspecified resource kind",
                ));
            }
            ResourceKind::GatewayClass
            | ResourceKind::EdgionGatewayConfig
            | ResourceKind::Gateway => {
                return Err(tonic::Status::invalid_argument(
                    "Base conf resources should not be synced via list",
                ));
            }
            ResourceKind::HTTPRoute => {
                apply_resource_list::<HTTPRoute>(
                    hub,
                    kind,
                    &list_response.data,
                    list_response.resource_version,
                )?;
            }
            ResourceKind::Service => {
                apply_resource_list::<Service>(
                    hub,
                    kind,
                    &list_response.data,
                    list_response.resource_version,
                )?;
            }
            ResourceKind::EndpointSlice => {
                apply_resource_list::<EndpointSlice>(
                    hub,
                    kind,
                    &list_response.data,
                    list_response.resource_version,
                )?;
            }
            ResourceKind::EdgionTls => {
                apply_resource_list::<EdgionTls>(
                    hub,
                    kind,
                    &list_response.data,
                    list_response.resource_version,
                )?;
            }
            ResourceKind::Secret => {
                apply_resource_list::<Secret>(
                    hub,
                    kind,
                    &list_response.data,
                    list_response.resource_version,
                )?;
            }
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
                    ResourceKind::Unspecified => {
                        tracing::warn!(kind = ?kind_clone, "Unspecified resource kind cannot be watched");
                        return;
                    }
                    // Base conf resources should not be watched
                    ResourceKind::GatewayClass
                    | ResourceKind::EdgionGatewayConfig
                    | ResourceKind::Gateway => {
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
                let mut client = match Self::create_client_internal(
                    &grpc_server_addr,
                    grpc_server_connect_timeout,
                )
                .await
                {
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
                    kind: kind_clone as i32,
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

                            // Parse the events from the watch response (JSON format from gRPC)
                            let events: Vec<serde_json::Value> = match serde_json::from_str(
                                &watch_response.data,
                            ) {
                                Ok(events) => events,
                                Err(e) => {
                                    tracing::error!(error = %e, "Failed to parse watch response events");
                                    continue;
                                }
                            };

                            let hub = &hub_clone;
                            for event in events {
                                if let Some(event_type) = event.get("type").and_then(|v| v.as_str())
                                {
                                    // Convert JSON to YAML for application layer
                                    let data_str = match serde_yaml::to_string(
                                        &event.get("data").unwrap_or(&serde_json::Value::Null),
                                    ) {
                                        Ok(s) => s,
                                        Err(e) => {
                                            tracing::error!(error = %e, "Failed to convert event data JSON to YAML");
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
        let request = tonic::Request::new(ListRequest {
            key,
            kind: kind as i32,
        });

        let response = self.conf_client_handle.list(request).await?;
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
        tracing::info!(
            key = %key,
            kind = ?kind,
            client_id = %client_id,
            client_name = %client_name,
            "Start watch"
        );
        let request = tonic::Request::new(WatchRequest {
            key,
            kind: kind as i32,
            client_id,
            client_name,
            from_version,
        });

        let mut stream = self.conf_client_handle.watch(request).await?.into_inner();

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

fn apply_resource_list<T>(
    hub: &Arc<ConfigClient>,
    kind: ResourceKind,
    data: &str,
    resource_version: u64,
) -> Result<(), tonic::Status>
where
    T: DeserializeOwned + Serialize,
{
    let resources: Vec<T> = serde_json::from_str(data).map_err(|e| {
        tracing::error!(
            kind = ?kind,
            error = %e,
            data_preview = %&data[..data.len().min(200)],
            "Failed to parse list response"
        );
        tonic::Status::internal(format!("Failed to parse list response: {}", e))
    })?;

    tracing::info!(kind = ?kind, count = resources.len(), "Parsed resources");

    for (idx, resource) in resources.into_iter().enumerate() {
        let data_str = serde_yaml::to_string(&resource).map_err(|e| {
            tracing::error!(
                kind = ?kind,
                index = idx,
                error = %e,
                "Failed to convert resource to YAML"
            );
            tonic::Status::internal(format!("Failed to convert resource to YAML: {}", e))
        })?;

        hub.apply_resource_change(
            ResourceChange::InitAdd,
            Some(kind),
            data_str,
            Some(resource_version),
        );
    }

    Ok(())
}
