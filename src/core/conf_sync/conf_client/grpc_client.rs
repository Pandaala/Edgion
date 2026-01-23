use crate::core::conf_sync::conf_client::ConfigClient;
use crate::core::conf_sync::proto::config_sync_client::ConfigSyncClient as ConfigSyncClientService;
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
    #[allow(dead_code)]
    conf_client_handle: ConfigSyncClientService<Channel>,
}

impl ConfigSyncClient {
    /// Create a new ConfigSync conf_client and connect to conf_server_old
    /// Uses lazy connection to allow cold start even if the server is not available immediately.
    ///
    /// # Arguments
    /// * `grpc_server_addr` - Address of the gRPC server
    /// * `client_name` - Human-readable name for this client
    /// * `timeout` - Connection and request timeout
    pub async fn new(
        grpc_server_addr: &str,
        client_name: String,
        timeout: Duration,
    ) -> Result<Self, tonic::transport::Error> {
        let client_id = Uuid::new_v4().to_string();
        let config_client = Arc::new(ConfigClient::new(client_id.clone(), client_name.clone()));

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
        config_client.gateway_classes().set_grpc_client(client.clone()).await;
        config_client.gateways().set_grpc_client(client.clone()).await;
        config_client
            .edgion_gateway_configs()
            .set_grpc_client(client.clone())
            .await;
        config_client.routes().set_grpc_client(client.clone()).await;
        config_client.grpc_routes().set_grpc_client(client.clone()).await;
        config_client.tcp_routes().set_grpc_client(client.clone()).await;
        config_client.udp_routes().set_grpc_client(client.clone()).await;
        config_client.tls_routes().set_grpc_client(client.clone()).await;
        config_client.link_sys().set_grpc_client(client.clone()).await;
        config_client.services().set_grpc_client(client.clone()).await;
        config_client.endpoint_slices().set_grpc_client(client.clone()).await;
        config_client.endpoints().set_grpc_client(client.clone()).await;
        config_client.edgion_tls().set_grpc_client(client.clone()).await;
        config_client.edgion_plugins().set_grpc_client(client.clone()).await;
        config_client
            .edgion_stream_plugins()
            .set_grpc_client(client.clone())
            .await;
        config_client.reference_grants().set_grpc_client(client.clone()).await;
        config_client
            .backend_tls_policies()
            .set_grpc_client(client.clone())
            .await;
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

    /// Start watching a specific resource kind and automatically sync to ConfigHub
    pub async fn start_watch_sync(&mut self, _key: String, kind: ResourceKind) -> Result<(), tonic::Status> {
        match kind {
            Unspecified => {
                return Err(tonic::Status::invalid_argument(
                    "Cannot watch unspecified resource kind",
                ));
            }
            // Base conf resources can now be watched
            GatewayClass => {
                self.config_client.gateway_classes().start_watch().await?;
            }
            EdgionGatewayConfig => {
                self.config_client.edgion_gateway_configs().start_watch().await?;
            }
            Gateway => {
                self.config_client.gateways().start_watch().await?;
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
            Endpoint => {
                self.config_client.endpoints().start_watch().await?;
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
            BackendTLSPolicy => {
                self.config_client.backend_tls_policies().start_watch().await?;
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
        // Watch all resources including base_conf resources
        let resource_kinds = vec![
            GatewayClass,
            EdgionGatewayConfig,
            Gateway,
            HTTPRoute,
            GRPCRoute,
            TCPRoute,
            UDPRoute,
            TLSRoute,
            LinkSys,
            Service,
            EndpointSlice,
            Endpoint,
            EdgionTls,
            EdgionPlugins,
            EdgionStreamPlugins,
            ReferenceGrant,
            BackendTLSPolicy,
            PluginMetaData,
            Secret,
        ];

        for kind in resource_kinds {
            if let Err(e) = self.start_watch_sync(String::new(), kind).await {
                tracing::error!(kind = ?kind, error = %e, "Failed to start watch");
            }
        }

        Ok(())
    }
}
