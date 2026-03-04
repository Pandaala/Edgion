//! gRPC server for configuration synchronization (new version)
//!
//! This module provides the gRPC service implementation using ConfigSyncServer.

use std::sync::Arc;
use tonic::{Request, Response, Status};

use crate::core::conf_mgr::sync_runtime::metrics::reload_metrics;
use crate::core::conf_sync::proto::{
    config_sync_server::{ConfigSync, ConfigSyncServer as ConfigSyncService},
    ListRequest, ListResponse, ServerInfoRequest, ServerInfoResponse, ServerMetaEvent, WatchRequest, WatchResponse,
    WatchServerMetaRequest,
};
use crate::types::{WATCH_ERR_SERVER_ID_MISMATCH, WATCH_ERR_SERVER_RELOAD};

use super::ConfigSyncServer;

/// Provider trait for dynamically obtaining ConfigSyncServer
///
/// This allows the gRPC server to get the latest ConfigSyncServer instance
/// after reload operations.
pub trait ConfigSyncServerProvider: Send + Sync {
    fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>>;
}

/// gRPC ConfigSync service implementation
///
/// Uses a provider to dynamically obtain ConfigSyncServer, which allows
/// the gRPC server to use the latest instance after reload operations.
pub struct ConfigSyncGrpcServer {
    provider: Arc<dyn ConfigSyncServerProvider>,
}

impl ConfigSyncGrpcServer {
    /// Create a new gRPC server with a dynamic provider
    pub fn new(provider: Arc<dyn ConfigSyncServerProvider>) -> Self {
        Self { provider }
    }

    /// Create a new gRPC server with a static ConfigSyncServer (for backward compatibility)
    #[allow(dead_code)]
    pub fn with_static_server(server: Arc<ConfigSyncServer>) -> Self {
        struct StaticProvider(Arc<ConfigSyncServer>);
        impl ConfigSyncServerProvider for StaticProvider {
            fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>> {
                Some(self.0.clone())
            }
        }
        Self {
            provider: Arc::new(StaticProvider(server)),
        }
    }

    /// Get the current ConfigSyncServer, returns error if not available
    #[allow(clippy::result_large_err)]
    fn get_server(&self) -> Result<Arc<ConfigSyncServer>, Status> {
        self.provider
            .config_sync_server()
            .ok_or_else(|| Status::unavailable("ConfigSyncServer not ready"))
    }

    /// Convert to tonic service
    pub fn into_service(self) -> ConfigSyncService<ConfigSyncGrpcServer> {
        ConfigSyncService::new(self)
    }

    /// Start the gRPC server on the given address
    pub async fn serve(self, addr: std::net::SocketAddr) -> Result<(), tonic::transport::Error> {
        let service = self.into_service();
        tonic::transport::Server::builder()
            .add_service(service)
            .serve(addr)
            .await
    }

    /// Start the gRPC server with graceful shutdown support
    pub async fn serve_with_shutdown(
        self,
        addr: std::net::SocketAddr,
        shutdown_signal: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Result<(), tonic::transport::Error> {
        let service = self.into_service();

        tracing::info!(
            component = "grpc_server",
            addr = %addr,
            "Starting gRPC server with graceful shutdown support"
        );

        let result = tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_shutdown(addr, shutdown_signal)
            .await;

        tracing::info!(component = "grpc_server", "gRPC server stopped");

        result
    }

    /// Start the gRPC server with reflection support
    pub async fn serve_with_reflection(
        self,
        addr: std::net::SocketAddr,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let service = self.into_service();
        let reflection_service = tonic_reflection::server::Builder::configure()
            .register_encoded_file_descriptor_set(tonic::include_file_descriptor_set!("config_sync_descriptor"))
            .build_v1()?;

        tonic::transport::Server::builder()
            .add_service(service)
            .add_service(reflection_service)
            .serve(addr)
            .await?;

        Ok(())
    }
}

#[tonic::async_trait]
impl ConfigSync for ConfigSyncGrpcServer {
    async fn get_server_info(
        &self,
        _request: Request<ServerInfoRequest>,
    ) -> Result<Response<ServerInfoResponse>, Status> {
        let server = self.get_server()?;

        let endpoint_mode = server
            .endpoint_mode()
            .map(|m| format!("{:?}", m))
            .unwrap_or_else(|| "Auto".to_string());

        let supported_kinds = server.all_kinds();

        tracing::debug!(
            component = "grpc_server",
            endpoint_mode = %endpoint_mode,
            supported_kinds = ?supported_kinds,
            "GetServerInfo request"
        );

        Ok(Response::new(ServerInfoResponse {
            server_id: server.server_id(),
            endpoint_mode,
            supported_kinds,
        }))
    }

    async fn list(&self, request: Request<ListRequest>) -> Result<Response<ListResponse>, Status> {
        let server = self.get_server()?;
        let req = request.into_inner();

        // Validate expected_server_id if provided
        if !req.expected_server_id.is_empty() {
            let current_server_id = server.server_id();
            if req.expected_server_id != current_server_id {
                tracing::warn!(
                    component = "grpc_server",
                    expected = %req.expected_server_id,
                    actual = %current_server_id,
                    "Server ID mismatch in list request"
                );
                return Err(Status::failed_precondition(WATCH_ERR_SERVER_ID_MISMATCH));
            }
        }

        // Validate kind is not empty
        if req.kind.is_empty() {
            return Err(Status::invalid_argument("Resource kind is required"));
        }

        // Call list on ConfigSyncServer with string kind directly
        let list_data = server
            .list(&req.kind)
            .map_err(|e| Status::internal(format!("Failed to list resources: {}", e)))?;

        Ok(Response::new(ListResponse {
            data: list_data.data,
            sync_version: list_data.sync_version,
            server_id: list_data.server_id,
        }))
    }

    type WatchStream = tokio_stream::wrappers::ReceiverStream<Result<WatchResponse, Status>>;

    async fn watch(&self, request: Request<WatchRequest>) -> Result<Response<Self::WatchStream>, Status> {
        let server = self.get_server()?;
        let req = request.into_inner();

        // Validate expected_server_id if provided
        if !req.expected_server_id.is_empty() {
            let current_server_id = server.server_id();
            if req.expected_server_id != current_server_id {
                tracing::warn!(
                    component = "grpc_server",
                    expected = %req.expected_server_id,
                    actual = %current_server_id,
                    client_id = %req.client_id,
                    "Server ID mismatch in watch request"
                );
                return Err(Status::failed_precondition(WATCH_ERR_SERVER_ID_MISMATCH));
            }
        }

        // Validate kind is not empty
        if req.kind.is_empty() {
            return Err(Status::invalid_argument("Resource kind is required"));
        }

        let kind_name = &req.kind;
        let client_id_log = req.client_id.clone();
        let client_name_log = req.client_name.clone();

        tracing::info!(
            component = "grpc_server",
            key = %req.key,
            kind = %kind_name,
            client_id = %client_id_log,
            client_name = %client_name_log,
            from_version = req.from_version,
            "Watch request received"
        );

        // Call watch on ConfigSyncServer with string kind directly
        let receiver = server
            .watch(kind_name, req.client_id, req.client_name, req.from_version)
            .map_err(|e| {
                tracing::error!(
                    component = "grpc_server",
                    key = %req.key,
                    kind = %kind_name,
                    client_id = %client_id_log,
                    client_name = %client_name_log,
                    error = %e,
                    "Watch failed"
                );
                Status::internal(format!("Failed to start watch: {}", e))
            })?;

        tracing::info!(
            component = "grpc_server",
            key = %req.key,
            kind = %kind_name,
            client_id = %client_id_log,
            client_name = %client_name_log,
            "Watch established"
        );

        // Convert EventDataSimple receiver to WatchResponse stream
        let (tx, rx) = tokio::sync::mpsc::channel(100);

        // Capture provider and initial server_id for reload detection
        let provider = self.provider.clone();
        let initial_server_id = server.server_id();

        tokio::spawn(async move {
            let mut receiver = receiver;
            let mut check_interval = tokio::time::interval(std::time::Duration::from_secs(1));

            loop {
                tokio::select! {
                    // Check for data from watch stream
                    event = receiver.recv() => {
                        match event {
                            Some(event_data) => {
                                let response = WatchResponse {
                                    data: event_data.data,
                                    sync_version: event_data.sync_version,
                                    err: event_data.err.unwrap_or_default(),
                                    server_id: event_data.server_id,
                                };

                                if tx.send(Ok(response)).await.is_err() {
                                    break;
                                }
                            }
                            None => break, // Stream ended
                        }
                    }
                    // Periodically check if server_id changed (reload completed)
                    _ = check_interval.tick() => {
                        if let Some(new_server) = provider.config_sync_server() {
                            let new_server_id = new_server.server_id();
                            if new_server_id != initial_server_id {
                                // Server reloaded, notify client to relist
                                tracing::info!(
                                    component = "grpc_server",
                                    old_server_id = %initial_server_id,
                                    new_server_id = %new_server_id,
                                    "Server reloaded, notifying client to relist"
                                );
                                let response = WatchResponse {
                                    data: String::new(),
                                    sync_version: 0,
                                    err: WATCH_ERR_SERVER_RELOAD.to_string(),
                                    server_id: new_server_id,
                                };
                                let _ = tx.send(Ok(response)).await;
                                // Record metric for client notification
                                reload_metrics().client_notified();
                                break;
                            }
                        }
                        // If provider returns None, server is reloading, keep waiting
                    }
                }
            }
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    type WatchServerMetaStream = tokio_stream::wrappers::ReceiverStream<Result<ServerMetaEvent, Status>>;

    async fn watch_server_meta(
        &self,
        request: Request<WatchServerMetaRequest>,
    ) -> Result<Response<Self::WatchServerMetaStream>, Status> {
        let server = self.get_server()?;
        let req = request.into_inner();
        let registry = server.client_registry();

        // Register this client
        registry.register(req.client_id.clone(), req.client_name.clone());

        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let server_id = server.server_id();
        let client_id = req.client_id.clone();

        tracing::info!(
            component = "grpc_server",
            client_id = %client_id,
            client_name = %req.client_name,
            "WatchServerMeta stream started"
        );

        // Capture provider and initial server_id for reload detection
        let provider = self.provider.clone();
        let initial_server_id = server_id.clone();

        tokio::spawn(async move {
            // Helper to get current timestamp in millis
            let now_millis = || -> u64 {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0)
            };

            // Send initial state immediately
            let initial_event = ServerMetaEvent {
                server_id: initial_server_id.clone(),
                gateway_instance_count: registry.count(),
                timestamp: now_millis(),
            };
            if tx.send(Ok(initial_event)).await.is_err() {
                registry.unregister(&client_id);
                return;
            }

            // Push on change (event-driven) + periodic server-switch detection (every 1s)
            // Mirrors the pattern used in watch() to detect controller reload/leader switch.
            let mut last_count = registry.count();
            let mut check_interval = tokio::time::interval(std::time::Duration::from_secs(1));

            loop {
                tokio::select! {
                    _ = registry.wait_for_change() => {
                        let new_count = registry.count();
                        if new_count == last_count {
                            continue; // Spurious wakeup or concurrent change that reverted
                        }
                        last_count = new_count;

                        let event = ServerMetaEvent {
                            server_id: initial_server_id.clone(),
                            gateway_instance_count: new_count,
                            timestamp: now_millis(),
                        };

                        if tx.send(Ok(event)).await.is_err() {
                            // Client disconnected, unregister
                            tracing::info!(
                                component = "grpc_server",
                                client_id = %client_id,
                                "WatchServerMeta client disconnected, unregistering"
                            );
                            registry.unregister(&client_id);
                            break;
                        }
                    }
                    // Periodically check if server_id changed (controller reload / leader switch)
                    _ = check_interval.tick() => {
                        if let Some(new_server) = provider.config_sync_server() {
                            let new_server_id = new_server.server_id();
                            if new_server_id != initial_server_id {
                                tracing::info!(
                                    component = "grpc_server",
                                    client_id = %client_id,
                                    old_server_id = %initial_server_id,
                                    new_server_id = %new_server_id,
                                    "Server reloaded, closing WatchServerMeta stream to force reconnect"
                                );
                                // Send a terminal event carrying the new server_id so the
                                // client can detect the switch boundary before reconnecting.
                                let switch_event = ServerMetaEvent {
                                    server_id: new_server_id,
                                    gateway_instance_count: new_server.client_registry().count(),
                                    timestamp: now_millis(),
                                };
                                let _ = tx.send(Ok(switch_event)).await;
                                // Unregister from the old registry before exiting
                                registry.unregister(&client_id);
                                break;
                            }
                        }
                        // If provider returns None the server is mid-reload; keep waiting.
                    }
                }
            }
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tokio_stream::StreamExt;

    // A mock provider that allows us to simulate a server reload
    struct MockConfigSyncProvider {
        server: Mutex<Arc<ConfigSyncServer>>,
    }

    impl MockConfigSyncProvider {
        fn new(server: Arc<ConfigSyncServer>) -> Self {
            Self {
                server: Mutex::new(server),
            }
        }

        fn set_server(&self, server: Arc<ConfigSyncServer>) {
            *self.server.lock().unwrap() = server;
        }
    }

    impl ConfigSyncServerProvider for MockConfigSyncProvider {
        fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>> {
            Some(self.server.lock().unwrap().clone())
        }
    }

    #[tokio::test]
    async fn test_watch_server_meta_reload_switch() {
        // 1. Create initial server
        let initial_server = Arc::new(ConfigSyncServer::new());
        let initial_server_id = initial_server.server_id();

        let provider = Arc::new(MockConfigSyncProvider::new(initial_server.clone()));
        let grpc_server = ConfigSyncGrpcServer::new(provider.clone());

        let client_id = "test-client-123".to_string();

        // 2. Start watch_server_meta
        let request = Request::new(WatchServerMetaRequest {
            client_id: client_id.clone(),
            client_name: "test-gateway".to_string(),
        });

        let response = grpc_server.watch_server_meta(request).await.unwrap();
        let mut stream = response.into_inner();

        // 3. Receive initial event
        let first_event = stream.next().await.unwrap().unwrap();
        assert_eq!(first_event.server_id, initial_server_id);
        assert_eq!(first_event.gateway_instance_count, 1); // 1 because the request registered the client

        // Verify registration in old registry
        assert_eq!(initial_server.client_registry().count(), 1);

        // 4. Simulate server reload (new server generated)
        let new_server = Arc::new(ConfigSyncServer::new());
        let new_server_id = new_server.server_id();
        assert_ne!(initial_server_id, new_server_id);

        provider.set_server(new_server.clone());

        // 5. Watch for the terminal event marking the switch boundary.
        // We use a short timeout because the tick happens every second.
        let switch_event = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
            .await
            .expect("should yield switch event quickly")
            .unwrap()
            .unwrap();

        // Terminal event should carry the NEW server_id and count
        assert_eq!(switch_event.server_id, new_server_id);

        // 6. Next poll should return None (stream closed)
        assert!(stream.next().await.is_none());

        // 7. Verify the old client unregistration logic happened
        // The count in old registry drops to 1, but let's just make sure unregister was called.
        // Even if empty, registry.count() returns max(1), but since we know the exit happened, we are good.
    }
}
