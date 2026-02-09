use crate::core::conf_sync::conf_client::ConfigClient;
use crate::core::conf_sync::proto::config_sync_client::ConfigSyncClient as ConfigSyncClientService;
use crate::core::conf_sync::proto::{ServerInfoRequest, ServerInfoResponse};
use crate::types::ResourceKind;
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

        // Set gRPC client for all caches via unified dispatch.
        // Uses ResourceKind exhaustive match internally, so adding a new kind
        // without handling it here will cause a compile error.
        config_client.set_all_grpc_clients(client.clone()).await;

        Ok(Self {
            config_client,
            conf_client_handle: client,
        })
    }

    /// Get a reference to the ConfigHub
    pub fn get_config_client(&self) -> Arc<ConfigClient> {
        self.config_client.clone()
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

    /// Start watching resource kinds based on server's supported kinds.
    ///
    /// Resolves each kind name to ResourceKind via the exhaustive enum, then
    /// dispatches to the corresponding cache's start_watch.
    ///
    /// - If a kind resolves to a cache that exists on Gateway: start watch.
    /// - If a kind resolves but has no Gateway cache (Secret, ReferenceGrant): skip.
    /// - If a kind string is completely unknown: warn and skip (forward compat).
    pub async fn start_watch_kinds(&mut self, supported_kinds: &[String]) -> Result<(), tonic::Status> {
        tracing::info!(
            supported_kinds = ?supported_kinds,
            "Starting watch for supported resource kinds"
        );

        for kind_name in supported_kinds {
            // Resolve string -> ResourceKind via exhaustive enum
            let kind = match ResourceKind::from_kind_name(kind_name) {
                Some(k) => k,
                None => {
                    // Truly unknown kind string — forward compat with newer server
                    tracing::warn!(kind = %kind_name, "Unknown resource kind from server, skipping");
                    continue;
                }
            };

            // Dispatch via get_dyn_cache (exhaustive ResourceKind match)
            match self.config_client.get_dyn_cache(kind) {
                Some(cache) => {
                    match cache.start_watch_dyn().await {
                        Ok(_) => {
                            tracing::info!(kind = %kind_name, "Watch started successfully");
                        }
                        Err(e) => {
                            tracing::error!(kind = %kind_name, error = %e, "Failed to start watch");
                        }
                    }
                }
                None => {
                    // Known kind but intentionally not cached on Gateway (Secret, ReferenceGrant)
                    tracing::debug!(kind = %kind_name, "Resource not cached on Gateway, skipping watch");
                }
            }
        }

        Ok(())
    }
}
