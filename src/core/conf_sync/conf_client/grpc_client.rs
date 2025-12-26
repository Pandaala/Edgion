use crate::core::conf_sync::conf_client::ConfigClient;
use crate::core::conf_sync::proto::{
    config_sync_client::ConfigSyncClient as ConfigSyncClientService, GetBaseConfRequest,
};
use crate::types::prelude_resources::*;
use crate::types::GatewayBaseConf;
use crate::types::ResourceKind::*;
use rand::Rng;
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
    /// Create a new ConfigSync conf_client and connect to conf_server
    /// Uses lazy connection to allow cold start even if the server is not available immediately.
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

        // TODO: Currently this allows cold start (gateway starts without controller).
        // However, without local cache persistence, the gateway will have no configuration
        // and cannot serve traffic until it successfully connects to the controller.
        // Future improvements should include:
        // 1. Persisting configuration to local disk (Snapshot)
        // 2. Loading from local disk on startup if controller is unreachable
        tracing::info!(
            server_addr = grpc_server_addr,
            "Initializing gRPC client with lazy connection"
        );

        // Ensure uri has scheme
        let uri = if grpc_server_addr.starts_with("http") {
            grpc_server_addr.to_string()
        } else {
            format!("http://{}", grpc_server_addr)
        };

        let endpoint = tonic::transport::Endpoint::from_shared(uri)?
            .timeout(timeout)
            .connect_timeout(timeout);

        // connect_lazy returns a Channel immediately, connection happens on first request
        let channel = endpoint.connect_lazy();
        let client = ConfigSyncClientService::new(channel);

        // Set gRPC conf_client for each cache immediately
        // Since it's a lazy channel, these calls won't fail due to connection issues
        config_client.routes().set_grpc_client(client.clone()).await;
        config_client.grpc_routes().set_grpc_client(client.clone()).await;
        config_client.tcp_routes().set_grpc_client(client.clone()).await;
        config_client.udp_routes().set_grpc_client(client.clone()).await;
        config_client.tls_routes().set_grpc_client(client.clone()).await;
        config_client.link_sys().set_grpc_client(client.clone()).await;
        config_client.services().set_grpc_client(client.clone()).await;
        config_client.endpoint_slices().set_grpc_client(client.clone()).await;
        config_client.edgion_tls().set_grpc_client(client.clone()).await;
        config_client.edgion_plugins().set_grpc_client(client.clone()).await;
        config_client.plugin_metadata().set_grpc_client(client.clone()).await;
        // config_client.secrets().set_grpc_client(client.clone()).await;

        Ok(Self {
            config_client,
            conf_client_handle: client,
        })
    }

    /// Get a reference to the ConfigHub
    pub fn get_config_client(&self) -> Arc<ConfigClient> {
        self.config_client.clone()
    }

    /// Fetch and initialize base configuration from conf_server
    /// This method will block and retry infinitely until the base configuration is successfully fetched.
    async fn fetch_and_init_base_conf(&mut self, gateway_class_key: &str) -> Result<(), tonic::Status> {
        // Loop until success
        loop {
            // Clone request for potential retries (Request is consumed by call)
            // Note: In tonic, Request cannot be easily cloned if it contains metadata/extensions that are not cloneable.
            // But here GetBaseConfRequest is a simple proto struct, so we can just recreate it.
            let req = tonic::Request::new(GetBaseConfRequest {
                gateway_class: gateway_class_key.to_string(),
            });

            match self.conf_client_handle.get_base_conf(req).await {
                Ok(response) => {
                    let base_conf_response = response.into_inner();
                    tracing::info!(base_conf_bytes = base_conf_response.base_conf.len(), "Init base_conf");

                    // Parse GatewayBaseConf
                    if !base_conf_response.base_conf.is_empty() {
                        match serde_json::from_str::<GatewayBaseConf>(&base_conf_response.base_conf) {
                            Ok(mut base_conf) => {
                                // Rebuild gateway_map after deserialization
                                base_conf.rebuild_gateway_map();

                                println!("[ConfigClient] Parsed GatewayBaseConf");
                                self.config_client.init_base_conf(base_conf);
                                tracing::info!("Base configuration initialized");
                                return Ok(());
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "Failed to parse base conf, will retry");
                                // If parsing fails, it might be a data issue, but we still retry in case it's transient
                                // or if we want to wait for a fix on the server side.
                            }
                        }
                    } else {
                        tracing::warn!("Received empty base configuration, will retry");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to fetch base conf, will retry");
                }
            }

            // Wait before retrying: 2s + Jitter
            let jitter_ms = rand::thread_rng().gen_range(0..1000);
            let sleep_duration = Duration::from_millis(2000 + jitter_ms);

            tracing::info!(
                retry_in_ms = sleep_duration.as_millis(),
                "Waiting for controller to become available..."
            );
            tokio::time::sleep(sleep_duration).await;
        }
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
            GRPCRoute => {
                self.config_client.grpc_routes().start_watch().await?;
            }
            TCPRoute => {
                self.config_client.tcp_routes().start_watch().await?;
            }
            UDPRoute => {
                self.config_client.udp_routes().start_watch().await?;
            }
            TLSRoute => {
                self.config_client.tls_routes().start_watch().await?;
            }
            LinkSys => {
                self.config_client.link_sys().start_watch().await?;
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
            EdgionStreamPlugins => {
                self.config_client.edgion_stream_plugins().start_watch().await?;
            }
            ReferenceGrant => {
                self.config_client.reference_grants().start_watch().await?;
            }
            PluginMetaData => {
                self.config_client.plugin_metadata().start_watch().await?;
            }
            Secret => {
                // Secret now follows related resources, not watched separately
                return Err(tonic::Status::invalid_argument(
                    "Secret resources are not watched separately",
                ));
            } // Secret => {
              //     self.config_client.secrets().start_watch().await?;
              // }
        }

        Ok(())
    }

    /// Start watching all resource types
    pub async fn start_watch_all(&mut self) -> Result<(), tonic::Status> {
        let hub = &self.config_client;
        let key = hub.get_gateway_class_key().clone();

        // Watch all non-base_conf resources
        let resource_kinds = vec![
            HTTPRoute,
            GRPCRoute,
            TCPRoute,
            UDPRoute,
            TLSRoute,
            LinkSys,
            Service,
            EndpointSlice,
            EdgionTls,
            EdgionPlugins,
            EdgionStreamPlugins,
            ReferenceGrant,
            PluginMetaData,
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
