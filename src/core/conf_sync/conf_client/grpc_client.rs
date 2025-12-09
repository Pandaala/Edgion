use crate::types::GatewayBaseConf;
use crate::core::conf_sync::conf_client::ConfigClient;
use crate::core::conf_sync::proto::{
    config_sync_client::ConfigSyncClient as ConfigSyncClientService, GetBaseConfRequest,
};
use crate::types::prelude_resources::*;
use crate::types::ResourceKind::*;
use std::sync::Arc;
use std::time::Duration;
use tonic::transport::Channel;
use tracing;
use uuid::Uuid;

/// gRPC conf_client for ConfigSync service
pub struct ConfigSyncClient {
    config_client: Arc<ConfigClient>,
    conf_client_handle: ConfigSyncClientService<Channel>,
}

impl ConfigSyncClient {
    /// Create a new ConfigSync conf_client and connect to conf_server with retry
    /// Retries connection up to 3 times with 2 second intervals
    pub async fn new(
        grpc_server_addr: &str,
        gateway_class_key: String,
        client_name: String,
        timeout: Duration,
    ) -> Result<Self, tonic::transport::Error> {
        let client_id = Uuid::new_v4().to_string();
        let config_client = Arc::new(ConfigClient::new(
            gateway_class_key,
            client_id.clone(),
            client_name.clone(),
        ));

        // Try to connect with retry logic: 3 attempts, 2 seconds apart
        const MAX_RETRIES: u32 = 3;
        const RETRY_INTERVAL_SECS: u64 = 2;

        let mut last_error = None;
        for attempt in 1..=MAX_RETRIES {
            tracing::info!(
                attempt = attempt,
                max_retries = MAX_RETRIES,
                server_addr = grpc_server_addr,
                "Attempting to connect to gRPC conf_server"
            );

            match Self::create_client_internal(grpc_server_addr, timeout).await {
                Ok(client) => {
                    tracing::info!(server_addr = grpc_server_addr, "Successfully connected to gRPC conf_server");

                    // Set gRPC conf_client for each cache
                    config_client.routes().set_grpc_client(client.clone()).await;
                    config_client.services().set_grpc_client(client.clone()).await;
                    config_client.endpoint_slices().set_grpc_client(client.clone()).await;
                    config_client.edgion_tls().set_grpc_client(client.clone()).await;
                    config_client.edgion_plugins().set_grpc_client(client.clone()).await;
                    config_client.secrets().set_grpc_client(client.clone()).await;

                    return Ok(Self {
                        config_client,
                        conf_client_handle: client,
                    });
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < MAX_RETRIES {
                        tracing::warn!(attempt = attempt,max_retries = MAX_RETRIES,error = %last_error.as_ref().unwrap(),
                            retry_in_secs = RETRY_INTERVAL_SECS,"Failed to connect, will retry");
                        tokio::time::sleep(Duration::from_secs(RETRY_INTERVAL_SECS)).await;
                    }
                }
            }
        }

        let err = last_error.unwrap();
        tracing::error!(server_addr = grpc_server_addr,error = %err,"Failed to connect to gRPC conf_server after {} attempts", MAX_RETRIES);
        Err(err)
    }

    /// Internal helper to create a conf_client
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

    /// Fetch and initialize base configuration from conf_server
    async fn fetch_and_init_base_conf(&mut self, gateway_class_key: &str) -> Result<(), tonic::Status> {
        let request = tonic::Request::new(GetBaseConfRequest {
            gateway_class: gateway_class_key.to_string(),
        });

        let response = self.conf_client_handle.get_base_conf(request).await?;
        let base_conf_response = response.into_inner();

        tracing::info!(
            base_conf_bytes = base_conf_response.base_conf.len(),
            "Init base_conf"
        );

        // Parse GatewayBaseConf
        if !base_conf_response.base_conf.is_empty() {
            match serde_json::from_str::<GatewayBaseConf>(&base_conf_response.base_conf) {
                Ok(mut base_conf) => {
                    // Rebuild gateway_map after deserialization
                    base_conf.rebuild_gateway_map();
                    
                    println!("[ConfigClient] Parsed GatewayBaseConf");
                    self.config_client.init_base_conf(base_conf);
                    tracing::info!("Base configuration initialized");
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to parse base conf");
                    return Err(tonic::Status::internal(format!("Failed to parse base conf: {}", e)));
                }
            }
        } else {
            tracing::warn!("Received empty base configuration");
        }

        Ok(())
    }

    pub async fn init_base_conf(&mut self) -> Result<(), tonic::Status> {
        let key = self.config_client.get_gateway_class_key().clone();
        self.fetch_and_init_base_conf(&key).await?;
        tracing::info!("Base Conf Initialized.");
        
        // Print base_conf as pretty JSON for debugging
        if let Some(base_conf) = self.config_client.get_base_conf() {
            match serde_json::to_string_pretty(&base_conf) {
                Ok(json) => {
                    tracing::info!("Base Configuration:\n{}", json);
                }
                Err(e) => {
                    tracing::warn!("Failed to serialize base_conf to JSON: {}", e);
                }
            }
        }
        
        Ok(())
    }

    /// Start watching a specific resource kind and automatically sync to ConfigHub
    pub async fn start_watch_sync(&mut self, _key: String, kind: ResourceKind) -> Result<(), tonic::Status> {
        match kind {
            Unspecified => {
                return Err(tonic::Status::invalid_argument(
                    "Cannot watch unspecified resource kind",
                ));
            }
            // Base conf resources should not be watched
            GatewayClass | EdgionGatewayConfig | Gateway => {
                return Err(tonic::Status::invalid_argument(
                    "Base conf resources should not be watched",
                ));
            }
            HTTPRoute => {
                self.config_client.routes().start_watch().await?;
            }
            Service => {
                self.config_client.services().start_watch().await?;
            }
            EndpointSlice => {
                self.config_client.endpoint_slices().start_watch().await?;
            }
            EdgionTls => {
                self.config_client.edgion_tls().start_watch().await?;
            }
            EdgionPlugins => {
                self.config_client.edgion_plugins().start_watch().await?;
            }
            Secret => {
                self.config_client.secrets().start_watch().await?;
            }
        }

        Ok(())
    }

    /// Start watching all resource types (kept for compatibility, but implementation removed)
    pub async fn start_watch_all(&mut self) -> Result<(), tonic::Status> {
        let hub = &self.config_client;
        let key = hub.get_gateway_class_key().clone();

        // Only watch non-base_conf resources
        let resource_kinds = vec![
            HTTPRoute,
            Service,
            EndpointSlice,
            EdgionTls,
            EdgionPlugins,
            Secret,
        ];
        
        for kind in resource_kinds {
            if let Err(e) = self.start_watch_sync(key.clone(), kind).await {
                tracing::error!(kind = ?kind, error = %e, "Failed to start watch");
            }
        }

        Ok(())
    }
}
