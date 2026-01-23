//! gRPC server for configuration synchronization
//!
//! This module provides the gRPC service implementation for ConfigSync.

use std::sync::Arc;
use tonic::{Request, Response, Status};

use crate::core::conf_mgr::ConfCenter;
use crate::core::conf_sync::proto::{
    config_sync_server::{ConfigSync, ConfigSyncServer as ConfigSyncService},
    ListRequest, ListResponse, WatchRequest, WatchResponse,
};
use crate::types::prelude_resources::*;
use crate::types::WATCH_ERR_SERVER_ID_MISMATCH;

use super::config_server::ConfigServer;

/// Server wrapper for ConfigSync gRPC service
///
/// Holds a reference to ConfCenter and dynamically gets ConfigServer.
/// When ConfigServer is None (during startup, relink, leadership loss),
/// returns UNAVAILABLE status.
pub struct ConfigSyncServer {
    conf_center: Arc<ConfCenter>,
}

impl ConfigSyncServer {
    pub fn new(conf_center: Arc<ConfCenter>) -> Self {
        Self { conf_center }
    }

    /// Get ConfigServer from ConfCenter, returns UNAVAILABLE if not ready
    #[allow(clippy::result_large_err)]
    fn get_config_server(&self) -> Result<Arc<ConfigServer>, Status> {
        self.conf_center
            .config_server()
            .ok_or_else(|| Status::unavailable("Server not ready - configuration sync in progress"))
    }

    pub fn into_service(self) -> ConfigSyncService<ConfigSyncServer> {
        ConfigSyncService::new(self)
    }

    /// Start the gRPC server on the given address
    pub async fn serve(self, addr: std::net::SocketAddr) -> Result<(), tonic::transport::Error> {
        let service = self.into_service();
        let server = tonic::transport::Server::builder().add_service(service).serve(addr);

        server.await
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

        let server = tonic::transport::Server::builder()
            .add_service(service)
            .add_service(reflection_service)
            .serve(addr);

        server.await?;
        Ok(())
    }
}

#[tonic::async_trait]
impl ConfigSync for ConfigSyncServer {
    async fn list(&self, request: Request<ListRequest>) -> Result<Response<ListResponse>, Status> {
        // Get ConfigServer (may be unavailable during startup/relink)
        let config_server = self.get_config_server()?;

        let req = request.into_inner();

        // Validate expected_server_id if provided
        if !req.expected_server_id.is_empty() {
            let current_server_id = config_server.server_id();
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

        // Convert incoming kind to ResourceKind
        let resource_kind =
            parse_resource_kind(req.kind).ok_or_else(|| Status::invalid_argument("Invalid resource kind"))?;

        // Call list on ConfigServer
        let list_data = config_server
            .list(&resource_kind)
            .map_err(|e| Status::internal(format!("Failed to list resources: {}", e)))?;

        Ok(Response::new(ListResponse {
            data: list_data.data,
            sync_version: list_data.sync_version,
            server_id: list_data.server_id,
        }))
    }

    type WatchStream = tokio_stream::wrappers::ReceiverStream<Result<WatchResponse, Status>>;

    async fn watch(&self, request: Request<WatchRequest>) -> Result<Response<Self::WatchStream>, Status> {
        // Get ConfigServer (may be unavailable during startup/relink)
        let config_server = self.get_config_server()?;

        let req = request.into_inner();

        // Validate expected_server_id if provided
        if !req.expected_server_id.is_empty() {
            let current_server_id = config_server.server_id();
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

        // Convert incoming kind to ResourceKind
        let resource_kind =
            parse_resource_kind(req.kind).ok_or_else(|| Status::invalid_argument("Invalid resource kind"))?;

        let client_id_log = req.client_id.clone();
        let client_name_log = req.client_name.clone();

        tracing::info!(
            component = "grpc_server",
            key = %req.key,
            kind = ?resource_kind,
            client_id = %client_id_log,
            client_name = %client_name_log,
            from_version = req.from_version,
            "Watch request received"
        );

        // Call watch on ConfigServer
        let watch_result = config_server.watch(&resource_kind, req.client_id, req.client_name, req.from_version);

        let receiver = match watch_result {
            Ok(receiver) => {
                tracing::info!(
                    component = "grpc_server",
                    key = %req.key,
                    kind = ?resource_kind,
                    client_id = %client_id_log,
                    client_name = %client_name_log,
                    "Watch established"
                );
                receiver
            }
            Err(e) => {
                tracing::error!(
                    component = "grpc_server",
                    key = %req.key,
                    kind = ?resource_kind,
                    client_id = %client_id_log,
                    client_name = %client_name_log,
                    error = %e,
                    "Watch failed"
                );
                return Err(Status::internal(format!("Failed to start watch: {}", e)));
            }
        };

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

fn parse_resource_kind(kind: i32) -> Option<ResourceKind> {
    ResourceKind::try_from(kind).ok().and_then(|k| match k {
        ResourceKind::Unspecified => None,
        _ => Some(k),
    })
}
