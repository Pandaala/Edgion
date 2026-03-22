//! gRPC server implementing FederationSync.
//!
//! On each connection:
//! 1. Wait up to 5s for first ControllerMessage (must be RegisterRequest)
//! 2. Register controller in registry
//! 3. Spawn heartbeat task (Ping every ping_interval)
//! 4. Loop: forward incoming messages to aggregator/commander; forward outgoing to stream

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tonic::{Request, Response, Status, Streaming};
use uuid::Uuid;

use crate::core::center::aggregator::ResourceAggregator;
use crate::core::center::config::CenterSyncConfig;
use crate::core::center::fed_sync::registry::ControllerRegistry;
use crate::core::common::fed_sync::proto::{
    center_message::Payload as CenterPayload,
    controller_message::Payload as CtrlPayload,
    federation_sync_server::FederationSync,
    CenterMessage, ControllerMessage, Ping, RegisterAck,
};

pub struct PendingListEntry {
    pub sender: oneshot::Sender<Vec<crate::core::common::fed_sync::proto::ResourceKey>>,
    pub created_at: std::time::Instant,
}
pub type PendingListMap = Arc<Mutex<HashMap<String, PendingListEntry>>>;
pub type PendingCommandMap = Arc<
    Mutex<
        HashMap<
            String,
            oneshot::Sender<crate::core::common::fed_sync::proto::CommandResponse>,
        >,
    >,
>;

pub struct FederationGrpcServer {
    pub registry: ControllerRegistry,
    pub aggregator: Arc<ResourceAggregator>,
    pub pending_lists: PendingListMap,
    pub pending_commands: PendingCommandMap,
    pub sync_config: CenterSyncConfig,
}

impl FederationGrpcServer {
    pub fn new(
        registry: ControllerRegistry,
        aggregator: Arc<ResourceAggregator>,
        sync_config: CenterSyncConfig,
    ) -> Self {
        Self {
            registry,
            aggregator,
            pending_lists: Arc::new(Mutex::new(HashMap::new())),
            pending_commands: Arc::new(Mutex::new(HashMap::new())),
            sync_config,
        }
    }
}

#[tonic::async_trait]
impl FederationSync for FederationGrpcServer {
    type SyncStream = tokio_stream::wrappers::ReceiverStream<Result<CenterMessage, Status>>;

    async fn sync(
        &self,
        request: Request<Streaming<ControllerMessage>>,
    ) -> Result<Response<Self::SyncStream>, Status> {
        let mut inbound = request.into_inner();
        let (out_tx, out_rx) = mpsc::channel::<Result<CenterMessage, Status>>(32);
        let (inner_tx, mut inner_rx) = mpsc::channel::<CenterMessage>(32);

        // 1. Wait for RegisterRequest (5s timeout)
        let first_msg = tokio::time::timeout(Duration::from_secs(5), inbound.message())
            .await
            .map_err(|_| {
                Status::deadline_exceeded(
                    "Registration timeout: no RegisterRequest within 5s",
                )
            })?
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::cancelled("Stream closed before RegisterRequest"))?;

        let register_req = match first_msg.payload {
            Some(CtrlPayload::Register(r)) => r,
            _ => {
                return Err(Status::invalid_argument(
                    "First message must be RegisterRequest",
                ))
            }
        };

        let controller_id = register_req.controller_id.clone();
        let session_id = Uuid::new_v4().to_string();

        tracing::info!(
            component = "fed_server",
            controller_id = %controller_id,
            session_id = %session_id,
            cluster = %register_req.cluster,
            "Controller registered"
        );

        // 2. Register in registry
        self.registry.register(
            controller_id.clone(),
            register_req,
            inner_tx.clone(),
            session_id.clone(),
        );

        // Send RegisterAck
        let _ = inner_tx
            .send(CenterMessage {
                payload: Some(CenterPayload::RegisterAck(RegisterAck { session_id })),
            })
            .await;

        let registry = self.registry.clone();
        let aggregator = self.aggregator.clone();
        let pending_lists = self.pending_lists.clone();
        let pending_commands = self.pending_commands.clone();
        let ping_interval = Duration::from_secs(self.sync_config.ping_interval_secs);
        let heartbeat_timeout = ping_interval * 3;
        let cid = controller_id.clone();

        // 3. Forward inner_rx → out_tx
        tokio::spawn({
            let out_tx = out_tx.clone();
            async move {
                while let Some(msg) = inner_rx.recv().await {
                    if out_tx.send(Ok(msg)).await.is_err() {
                        break;
                    }
                }
            }
        });

        // 4. Heartbeat task
        tokio::spawn({
            let inner_tx = inner_tx.clone();
            let cid = cid.clone();
            async move {
                let mut interval = tokio::time::interval(ping_interval);
                interval.tick().await; // skip first
                loop {
                    interval.tick().await;
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    if inner_tx
                        .send(CenterMessage {
                            payload: Some(CenterPayload::Ping(Ping {
                                timestamp: now_ms,
                            })),
                        })
                        .await
                        .is_err()
                    {
                        tracing::info!(
                            component = "fed_server",
                            controller_id = %cid,
                            "Heartbeat channel closed"
                        );
                        break;
                    }
                }
            }
        });

        // 5. Main message loop
        tokio::spawn({
            let registry = registry.clone();
            let cid = cid.clone();
            async move {
                loop {
                    match tokio::time::timeout(heartbeat_timeout, inbound.message()).await {
                        Err(_) => {
                            tracing::warn!(
                                component = "fed_server",
                                controller_id = %cid,
                                "Heartbeat timeout, marking offline"
                            );
                            registry.mark_offline(&cid);
                            break;
                        }
                        Ok(Err(e)) => {
                            tracing::info!(
                                component = "fed_server",
                                controller_id = %cid,
                                error = %e,
                                "Stream error"
                            );
                            registry.mark_offline(&cid);
                            break;
                        }
                        Ok(Ok(None)) => {
                            tracing::info!(
                                component = "fed_server",
                                controller_id = %cid,
                                "Stream closed"
                            );
                            registry.mark_offline(&cid);
                            break;
                        }
                        Ok(Ok(Some(msg))) => {
                            registry.update_last_seen(&cid);
                            match msg.payload {
                                Some(CtrlPayload::Pong(_)) => {}
                                Some(CtrlPayload::ListResponse(resp)) => {
                                    if let Some(entry) =
                                        pending_lists.lock().unwrap().remove(&resp.request_id)
                                    {
                                        let _ = entry.sender.send(resp.keys.clone());
                                    }
                                    aggregator.update_snapshot(&cid, resp.keys);
                                }
                                Some(CtrlPayload::CommandResponse(resp)) => {
                                    if let Some(sender) =
                                        pending_commands.lock().unwrap().remove(&resp.request_id)
                                    {
                                        let _ = sender.send(resp);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
            out_rx,
        )))
    }
}
