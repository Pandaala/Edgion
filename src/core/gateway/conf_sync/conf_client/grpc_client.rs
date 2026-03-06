use crate::core::common::conf_sync::proto::config_sync_client::ConfigSyncClient as ConfigSyncClientService;
use crate::core::common::conf_sync::proto::{ServerInfoRequest, ServerInfoResponse, WatchServerMetaRequest};
use crate::core::gateway::cli::config::set_gateway_instance_count;
use crate::core::gateway::conf_sync::conf_client::ConfigClient;
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
    /// Client ID for WatchServerMeta registration
    client_id: String,
    /// Client name for WatchServerMeta registration
    client_name: String,
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
            client_id,
            client_name,
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

    /// Start watching server metadata (gateway instance count, etc.)
    ///
    /// Runs as a background task. Updates the global GATEWAY_INSTANCE_COUNT
    /// via set_gateway_instance_count(). Automatically reconnects on failure.
    ///
    /// On server switch (server_id change mid-stream), reconnect immediately
    /// without backoff to ensure fast convergence. Exponential backoff is only
    /// applied for real connection/transport failures.
    ///
    /// This should be called once at startup via tokio::spawn.
    pub async fn start_watch_server_meta(self: Arc<Self>) {
        let client_id = self.client_id.clone();
        let client_name = self.client_name.clone();

        let mut backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(30);

        loop {
            tracing::info!("Starting WatchServerMeta stream...");

            match self
                .conf_client_handle
                .clone()
                .watch_server_meta(WatchServerMetaRequest {
                    client_id: client_id.clone(),
                    client_name: client_name.clone(),
                })
                .await
            {
                Ok(response) => {
                    backoff = Duration::from_secs(1); // Reset on successful connection
                    let mut stream = response.into_inner();

                    // Track the server_id seen on the first event of this stream session.
                    // If a subsequent event carries a different server_id it is the terminal
                    // server-switch signal from the server side (Stage A); reconnect immediately.
                    let mut stream_server_id: Option<String> = None;
                    let mut server_switched = false;

                    loop {
                        match stream.message().await {
                            Ok(Some(event)) => {
                                set_gateway_instance_count(event.gateway_instance_count);

                                if !event.server_id.is_empty() {
                                    match &stream_server_id {
                                        None => {
                                            // First event: record the authoritative server_id
                                            // and update the config client.
                                            stream_server_id = Some(event.server_id.clone());
                                            self.config_client.set_current_server_id(event.server_id);
                                        }
                                        Some(sid) if *sid != event.server_id => {
                                            // server_id changed mid-stream — server has reloaded.
                                            // Update config client with new server_id and reconnect
                                            // immediately (skip backoff sleep below).
                                            tracing::info!(
                                                old_server_id = %sid,
                                                new_server_id = %event.server_id,
                                                "WatchServerMeta: server switch detected, reconnecting immediately"
                                            );
                                            self.config_client.set_current_server_id(event.server_id);
                                            server_switched = true;
                                            break;
                                        }
                                        Some(_) => {
                                            // Same server_id — normal update.
                                            self.config_client.set_current_server_id(event.server_id);
                                        }
                                    }
                                }
                            }
                            Ok(None) => {
                                tracing::warn!("WatchServerMeta stream ended, reconnecting...");
                                break;
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    "WatchServerMeta stream error, reconnecting..."
                                );
                                break;
                            }
                        }
                    }

                    if server_switched {
                        // Intentional switch — skip backoff and reconnect immediately.
                        continue;
                    }
                }
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        backoff_secs = backoff.as_secs(),
                        "WatchServerMeta connection failed, retrying..."
                    );
                }
            }

            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(max_backoff); // Exponential backoff for real failures
        }
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
        let mut effective_kinds = supported_kinds.to_vec();
        for required in [
            "GatewayClass".to_string(),
            "Gateway".to_string(),
            "EdgionGatewayConfig".to_string(),
        ] {
            if !effective_kinds.iter().any(|k| k == &required) {
                effective_kinds.push(required);
            }
        }

        tracing::info!(
            supported_kinds = ?supported_kinds,
            effective_kinds = ?effective_kinds,
            "Starting watch for supported resource kinds"
        );
        self.config_client.set_required_kinds_for_readiness(&effective_kinds);

        for kind_name in &effective_kinds {
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
                Some(cache) => match cache.start_watch_dyn().await {
                    Ok(_) => {
                        tracing::info!(kind = %kind_name, "Watch started successfully");
                    }
                    Err(e) => {
                        tracing::error!(kind = %kind_name, error = %e, "Failed to start watch");
                    }
                },
                None => {
                    // Known kind but intentionally not cached on Gateway (Secret, ReferenceGrant)
                    tracing::debug!(kind = %kind_name, "Resource not cached on Gateway, skipping watch");
                }
            }
        }

        Ok(())
    }
}
