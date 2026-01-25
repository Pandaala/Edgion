//! gRPC server for configuration synchronization (new version)
//!
//! This module provides the gRPC service implementation using ConfigSyncServer.

use std::sync::Arc;
use tonic::{Request, Response, Status};

use crate::core::conf_sync::proto::{
    config_sync_server::{ConfigSync, ConfigSyncServer as ConfigSyncService},
    ListRequest, ListResponse, ServerInfoRequest, ServerInfoResponse, WatchRequest, WatchResponse,
};
use crate::types::prelude_resources::ResourceKind;
use crate::types::WATCH_ERR_SERVER_ID_MISMATCH;

use super::ConfigSyncServer;

/// gRPC ConfigSync service implementation
///
/// Wraps ConfigSyncServer and provides gRPC endpoints for list/watch.
pub struct ConfigSyncGrpcServer {
    server: Arc<ConfigSyncServer>,
}

impl ConfigSyncGrpcServer {
    pub fn new(server: Arc<ConfigSyncServer>) -> Self {
        Self { server }
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
        let endpoint_mode = self
            .server
            .endpoint_mode()
            .map(|m| format!("{:?}", m))
            .unwrap_or_else(|| "Auto".to_string());

        let supported_kinds = self.server.all_kinds();

        tracing::debug!(
            component = "grpc_server",
            endpoint_mode = %endpoint_mode,
            supported_kinds = ?supported_kinds,
            "GetServerInfo request"
        );

        Ok(Response::new(ServerInfoResponse {
            server_id: self.server.server_id(),
            endpoint_mode,
            supported_kinds,
        }))
    }

    async fn list(&self, request: Request<ListRequest>) -> Result<Response<ListResponse>, Status> {
        let req = request.into_inner();

        // Validate expected_server_id if provided
        if !req.expected_server_id.is_empty() {
            let current_server_id = self.server.server_id();
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

        // Convert proto ResourceKind to string kind name
        let kind_name =
            parse_resource_kind_to_name(req.kind).ok_or_else(|| Status::invalid_argument("Invalid resource kind"))?;

        // Call list on ConfigSyncServer
        let list_data = self
            .server
            .list(kind_name)
            .map_err(|e| Status::internal(format!("Failed to list resources: {}", e)))?;

        Ok(Response::new(ListResponse {
            data: list_data.data,
            sync_version: list_data.sync_version,
            server_id: list_data.server_id,
        }))
    }

    type WatchStream = tokio_stream::wrappers::ReceiverStream<Result<WatchResponse, Status>>;

    async fn watch(&self, request: Request<WatchRequest>) -> Result<Response<Self::WatchStream>, Status> {
        let req = request.into_inner();

        // Validate expected_server_id if provided
        if !req.expected_server_id.is_empty() {
            let current_server_id = self.server.server_id();
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

        // Convert proto ResourceKind to string kind name
        let kind_name =
            parse_resource_kind_to_name(req.kind).ok_or_else(|| Status::invalid_argument("Invalid resource kind"))?;

        let client_id_log = req.client_id.clone();
        let client_name_log = req.client_name.clone();

        tracing::info!(
            component = "grpc_server",
            key = %req.key,
            kind = kind_name,
            client_id = %client_id_log,
            client_name = %client_name_log,
            from_version = req.from_version,
            "Watch request received"
        );

        // Call watch on ConfigSyncServer
        let receiver = self
            .server
            .watch(kind_name, req.client_id, req.client_name, req.from_version)
            .map_err(|e| {
                tracing::error!(
                    component = "grpc_server",
                    key = %req.key,
                    kind = kind_name,
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
            kind = kind_name,
            client_id = %client_id_log,
            client_name = %client_name_log,
            "Watch established"
        );

        // Convert EventDataSimple receiver to WatchResponse stream
        let (tx, rx) = tokio::sync::mpsc::channel(100);

        tokio::spawn(async move {
            let mut receiver = receiver;
            while let Some(event_data) = receiver.recv().await {
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
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
}

/// Convert proto ResourceKind (i32) to string kind name
fn parse_resource_kind_to_name(kind: i32) -> Option<&'static str> {
    let resource_kind = ResourceKind::try_from(kind).ok()?;

    match resource_kind {
        ResourceKind::Unspecified => None,
        // Handle special case: ResourceKind::Endpoint maps to "Endpoints" in kind_names
        ResourceKind::Endpoint => Some("Endpoints"),
        // For all others, use as_str() which returns the enum variant name
        _ => Some(resource_kind.as_str()),
    }
}
