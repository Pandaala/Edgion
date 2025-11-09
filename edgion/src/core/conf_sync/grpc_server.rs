use std::convert::TryFrom;
use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::{Request, Response, Status};

use crate::core::conf_sync::config_server::ConfigServer;
use crate::core::conf_sync::proto::{
    config_sync_server::{ConfigSync, ConfigSyncServer as ConfigSyncService},
    ListRequest, ListResponse, ResourceKind as ProtoResourceKind, WatchRequest, WatchResponse,
};
use crate::types::ResourceKind;

/// Server wrapper for WatcherMgr
pub struct ConfigSyncServer {
    config_center: Arc<Mutex<ConfigServer>>,
}

impl ConfigSyncServer {
    pub fn new(watcher_mgr: ConfigServer) -> Self {
        Self {
            config_center: Arc::new(Mutex::new(watcher_mgr)),
        }
    }

    /// Create a new ConfigSyncServer with a shared ConfigCenter
    pub fn new_with_shared(config_center: Arc<Mutex<ConfigServer>>) -> Self {
        Self { config_center }
    }

    /// Get a reference to the shared ConfigCenter
    pub fn get_config_center(&self) -> Arc<Mutex<ConfigServer>> {
        self.config_center.clone()
    }

    pub fn into_service(self) -> ConfigSyncService<ConfigSyncServer> {
        ConfigSyncService::new(self)
    }

    /// Start the gRPC server on the given address
    pub async fn serve(self, addr: std::net::SocketAddr) -> Result<(), tonic::transport::Error> {
        let service = self.into_service();
        let server = tonic::transport::Server::builder()
            .add_service(service)
            .serve(addr);

        server.await
    }

    /// Start the gRPC server with reflection support
    pub async fn serve_with_reflection(
        self,
        addr: std::net::SocketAddr,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let service = self.into_service();
        let reflection_service = tonic_reflection::server::Builder::configure()
            .register_encoded_file_descriptor_set(tonic::include_file_descriptor_set!(
                "config_sync_descriptor"
            ))
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
        let req = request.into_inner();

        // Convert proto ResourceKind to our ResourceKind
        let proto_kind = ProtoResourceKind::try_from(req.kind)
            .map_err(|_| Status::invalid_argument("Invalid resource kind"))?;
        let resource_kind = proto_to_resource_kind(proto_kind)
            .ok_or_else(|| Status::invalid_argument("Invalid resource kind"))?;

        // Get WatcherMgr and call list
        let watcher_mgr = self.config_center.lock().await;
        let list_data = watcher_mgr
            .list(&req.key, &resource_kind)
            .await
            .map_err(|e| Status::internal(format!("Failed to list resources: {}", e)))?;

        Ok(Response::new(ListResponse {
            data: list_data.data,
            resource_version: list_data.resource_version,
        }))
    }

    type WatchStream = tokio_stream::wrappers::ReceiverStream<Result<WatchResponse, Status>>;

    async fn watch(
        &self,
        request: Request<WatchRequest>,
    ) -> Result<Response<Self::WatchStream>, Status> {
        let req = request.into_inner();

        // Convert proto ResourceKind to our ResourceKind
        let proto_kind = ProtoResourceKind::try_from(req.kind)
            .map_err(|_| Status::invalid_argument("Invalid resource kind"))?;
        let resource_kind = proto_to_resource_kind(proto_kind)
            .ok_or_else(|| Status::invalid_argument("Invalid resource kind"))?;

        let client_id_log = req.client_id.clone();
        let client_name_log = req.client_name.clone();

        println!(
            "[ConfigSyncServer::watch] request key={} kind={:?} client_id={} client_name={} from_version={}",
            req.key,
            resource_kind,
            client_id_log,
            client_name_log,
            req.from_version
        );

        // Get WatcherMgr and call watch
        let mut watcher_mgr = self.config_center.lock().await;
        let watch_result = watcher_mgr.watch(
            &req.key,
            &resource_kind,
            req.client_id,
            req.client_name,
            req.from_version,
        );

        let receiver = match watch_result {
            Ok(receiver) => {
                println!(
                    "[ConfigSyncServer::watch] watch established key={} kind={:?} client_id={} client_name={}",
                    req.key,
                    resource_kind,
                    client_id_log,
                    client_name_log
                );
                receiver
            }
            Err(e) => {
                println!(
                    "[ConfigSyncServer::watch] watch failed key={} kind={:?} client_id={} client_name={} error={}",
                    req.key,
                    resource_kind,
                    client_id_log,
                    client_name_log,
                    e
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
                    resource_version: event_data.resource_version,
                    err: event_data.err.unwrap_or_default(),
                };

                if tx.send(Ok(response)).await.is_err() {
                    break;
                }
            }
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
            rx,
        )))
    }
}

/// Convert proto ResourceKind to our ResourceKind
fn proto_to_resource_kind(kind: ProtoResourceKind) -> Option<ResourceKind> {
    match kind {
        ProtoResourceKind::Unspecified => None,
        ProtoResourceKind::GatewayClass => Some(ResourceKind::GatewayClass),
        ProtoResourceKind::GatewayClassSpec => Some(ResourceKind::EdgionGatewayConfig),
        ProtoResourceKind::Gateway => Some(ResourceKind::Gateway),
        ProtoResourceKind::HttpRoute => Some(ResourceKind::HTTPRoute),
        ProtoResourceKind::Service => Some(ResourceKind::Service),
        ProtoResourceKind::EndpointSlice => Some(ResourceKind::EndpointSlice),
        ProtoResourceKind::EdgionTls => Some(ResourceKind::EdgionTls),
        ProtoResourceKind::Secret => Some(ResourceKind::Secret),
    }
}
