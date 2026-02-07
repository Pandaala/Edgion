use crate::core::conf_sync::conf_client::ConfigClient;
use crate::core::conf_sync::proto::config_sync_client::ConfigSyncClient as ConfigSyncClientService;
use crate::core::conf_sync::proto::{ServerInfoRequest, ServerInfoResponse};
use std::sync::Arc;
use std::time::Duration;
use tonic::transport::Channel;
use tracing;
use uuid::Uuid;

/// Error type for watch start operations
enum WatchStartError {
    /// Unknown resource kind (client doesn't recognize it)
    UnknownKind,
    /// Watch skipped for a known reason
    Skipped(&'static str),
    /// Watch failed with an error
    Failed(tonic::Status),
}

impl From<tonic::Status> for WatchStartError {
    fn from(e: tonic::Status) -> Self {
        WatchStartError::Failed(e)
    }
}

/// gRPC conf_client for ConfigSync service
pub struct ConfigSyncClient {
    config_client: Arc<ConfigClient>,
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
        config_client.edgion_acme().set_grpc_client(client.clone()).await;
        config_client.edgion_plugins().set_grpc_client(client.clone()).await;
        config_client
            .edgion_stream_plugins()
            .set_grpc_client(client.clone())
            .await;
        // ReferenceGrant is not synced to Gateway - validation is done on Controller
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

    /// Try to start watch for a specific resource kind by name
    ///
    /// Returns Ok(()) if watch started successfully, or WatchStartError otherwise.
    async fn try_start_watch_for_kind(&self, kind: &str) -> Result<(), WatchStartError> {
        match kind {
            "GatewayClass" => self.config_client.gateway_classes().start_watch().await?,
            "EdgionGatewayConfig" => self.config_client.edgion_gateway_configs().start_watch().await?,
            "Gateway" => self.config_client.gateways().start_watch().await?,
            "HTTPRoute" => self.config_client.routes().start_watch().await?,
            "GRPCRoute" => self.config_client.grpc_routes().start_watch().await?,
            "TCPRoute" => self.config_client.tcp_routes().start_watch().await?,
            "UDPRoute" => self.config_client.udp_routes().start_watch().await?,
            "TLSRoute" => self.config_client.tls_routes().start_watch().await?,
            "LinkSys" => self.config_client.link_sys().start_watch().await?,
            "Service" => self.config_client.services().start_watch().await?,
            "EndpointSlice" => self.config_client.endpoint_slices().start_watch().await?,
            "Endpoints" => self.config_client.endpoints().start_watch().await?,
            "EdgionTls" => self.config_client.edgion_tls().start_watch().await?,
            "EdgionAcme" => self.config_client.edgion_acme().start_watch().await?,
            "EdgionPlugins" => self.config_client.edgion_plugins().start_watch().await?,
            "EdgionStreamPlugins" => self.config_client.edgion_stream_plugins().start_watch().await?,
            "BackendTLSPolicy" => self.config_client.backend_tls_policies().start_watch().await?,
            "PluginMetaData" => self.config_client.plugin_metadata().start_watch().await?,
            // Resources that should not be watched separately
            "ReferenceGrant" => return Err(WatchStartError::Skipped("not synced to Gateway")),
            "Secret" => return Err(WatchStartError::Skipped("follows related resources")),
            // Unknown kind - client doesn't recognize it (forward compatibility)
            _ => return Err(WatchStartError::UnknownKind),
        }
        Ok(())
    }

    /// Start watching all resource types (deprecated: use start_watch_kinds instead)
    #[allow(dead_code)]
    pub async fn start_watch_all(&mut self) -> Result<(), tonic::Status> {
        // Watch all resources including base_conf resources
        let all_kinds = vec![
            "GatewayClass",
            "EdgionGatewayConfig",
            "Gateway",
            "HTTPRoute",
            "GRPCRoute",
            "TCPRoute",
            "UDPRoute",
            "TLSRoute",
            "LinkSys",
            "Service",
            "EndpointSlice",
            "Endpoints",
            "EdgionTls",
            "EdgionAcme",
            "EdgionPlugins",
            "EdgionStreamPlugins",
            "ReferenceGrant",
            "BackendTLSPolicy",
            "PluginMetaData",
            "Secret",
        ];

        for kind in all_kinds {
            match self.try_start_watch_for_kind(kind).await {
                Ok(_) => {
                    tracing::info!(kind = kind, "Watch started successfully");
                }
                Err(WatchStartError::Skipped(reason)) => {
                    tracing::debug!(kind = kind, reason = reason, "Watch skipped");
                }
                Err(WatchStartError::UnknownKind) => {
                    tracing::warn!(kind = kind, "Unknown resource kind, skipping");
                }
                Err(WatchStartError::Failed(e)) => {
                    tracing::error!(kind = kind, error = %e, "Failed to start watch");
                }
            }
        }

        Ok(())
    }

    /// Get server information including endpoint mode and supported resource kinds
    pub async fn get_server_info(&mut self) -> Result<ServerInfoResponse, tonic::Status> {
        let response = self.conf_client_handle.get_server_info(ServerInfoRequest {}).await?;

        let info = response.into_inner();

        tracing::info!(
            server_id = %info.server_id,
            endpoint_mode = %info.endpoint_mode,
            supported_kinds = ?info.supported_kinds,
            "Received server info"
        );

        Ok(info)
    }

    /// Start watching resource kinds based on server's supported kinds
    ///
    /// This method filters the supported_kinds from server and starts watches
    /// only for the resources that the server actually supports.
    /// Unknown kinds are logged and skipped (forward compatibility with newer servers).
    pub async fn start_watch_kinds(&mut self, supported_kinds: &[String]) -> Result<(), tonic::Status> {
        tracing::info!(
            supported_kinds = ?supported_kinds,
            "Starting watch for supported resource kinds"
        );

        for kind_name in supported_kinds {
            match self.try_start_watch_for_kind(kind_name).await {
                Ok(_) => {
                    tracing::info!(kind = %kind_name, "Watch started successfully");
                }
                Err(WatchStartError::Skipped(reason)) => {
                    tracing::debug!(kind = %kind_name, reason = reason, "Watch skipped");
                }
                Err(WatchStartError::UnknownKind) => {
                    // Forward compatibility: old client doesn't recognize new resource types
                    tracing::warn!(kind = %kind_name, "Unknown resource kind from server, skipping");
                }
                Err(WatchStartError::Failed(e)) => {
                    tracing::error!(kind = %kind_name, error = %e, "Failed to start watch");
                }
            }
        }

        Ok(())
    }
}
