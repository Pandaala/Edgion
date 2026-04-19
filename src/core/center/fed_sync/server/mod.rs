//! gRPC server implementing FederationSync.
//!
//! On each connection:
//! 1. Wait up to 5s for first ControllerMessage (must be RegisterRequest)
//! 2. Register controller in registry
//! 3. Spawn heartbeat task (Ping every ping_interval)
//! 4. Loop: forward incoming messages to aggregator/commander; forward outgoing to stream

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tonic::{Request, Response, Status, Streaming};
use uuid::Uuid;

use crate::core::center::aggregator::ResourceAggregator;
use crate::core::center::config::CenterSyncConfig;
use crate::core::center::db::CenterDb;
use crate::core::center::fed_sync::registry::ControllerRegistry;
use crate::core::center::proxy::PendingProxyMap;
use crate::core::center::watch_cache::{CenterSyncClient, WatchEventSimple};
use crate::core::common::conf_sync::types::EventType;
use crate::core::common::fed_sync::proto::{
    center_message::Payload as CenterPayload, controller_message::Payload as CtrlPayload,
    federation_sync_server::FederationSync, CenterMessage, ControllerMessage, FedWatchRequest, Ping, RegisterAck,
};
use crate::types::resource::meta::ResourceMeta;
use crate::types::resources::plugin_metadata::PluginMetaData;

/// Typed representation of a single watch event from the controller.
#[derive(serde::Deserialize)]
struct WatchEventRaw {
    #[serde(rename = "type")]
    event_type: String,
    data: serde_json::Value,
    #[serde(default)]
    #[allow(dead_code)]
    sync_version: u64,
}

/// Per-kind watch state for a controller session.
/// Currently only PluginMetaData is watched; this struct makes it straightforward
/// to add more kinds by storing a `HashMap<kind, FedWatchState>`.
struct FedWatchState {
    /// Current request_id for correlation (stale responses are skipped).
    request_id: String,
    /// Controller's ConfigSyncServer instance ID (detects restarts).
    server_id: Option<String>,
    /// Consecutive error count (INFO on first, WARN on subsequent).
    consecutive_errors: u32,
}

impl FedWatchState {
    fn new(request_id: String, server_id: Option<String>) -> Self {
        Self {
            request_id,
            server_id,
            consecutive_errors: 0,
        }
    }

    /// Generate a new FedWatchRequest (from_version=0) and update internal request_id.
    fn re_watch(&mut self, kind: &str) -> CenterMessage {
        let new_id = Uuid::new_v4().to_string();
        self.request_id = new_id.clone();
        self.consecutive_errors = 0;
        CenterMessage {
            payload: Some(CenterPayload::WatchRequest(FedWatchRequest {
                request_id: new_id,
                kind: kind.to_string(),
                from_version: 0,
            })),
        }
    }
}

pub struct PendingListEntry {
    pub sender: oneshot::Sender<Vec<crate::core::common::fed_sync::proto::ResourceKey>>,
    pub created_at: std::time::Instant,
}
pub type PendingListMap = Arc<Mutex<HashMap<String, PendingListEntry>>>;
pub type PendingCommandMap =
    Arc<Mutex<HashMap<String, oneshot::Sender<crate::core::common::fed_sync::proto::CommandResponse>>>>;

pub struct FederationGrpcServer {
    pub registry: ControllerRegistry,
    pub aggregator: Arc<ResourceAggregator>,
    pub pending_lists: PendingListMap,
    pub pending_commands: PendingCommandMap,
    pub pending_proxies: PendingProxyMap,
    pub sync_config: CenterSyncConfig,
    pub sync_client: Arc<CenterSyncClient>,
    /// Optional — Center only persists controller registration when
    /// `database.enabled=true`. Absent DB means upsert/mark_offline are skipped.
    pub db: Option<Arc<CenterDb>>,
}

impl FederationGrpcServer {
    pub fn new(
        registry: ControllerRegistry,
        aggregator: Arc<ResourceAggregator>,
        pending_proxies: PendingProxyMap,
        sync_config: CenterSyncConfig,
        sync_client: Arc<CenterSyncClient>,
        db: Option<Arc<CenterDb>>,
    ) -> Self {
        Self {
            registry,
            aggregator,
            pending_lists: Arc::new(Mutex::new(HashMap::new())),
            pending_commands: Arc::new(Mutex::new(HashMap::new())),
            pending_proxies,
            sync_config,
            sync_client,
            db,
        }
    }
}

#[tonic::async_trait]
impl FederationSync for FederationGrpcServer {
    type SyncStream = tokio_stream::wrappers::ReceiverStream<Result<CenterMessage, Status>>;

    async fn sync(&self, request: Request<Streaming<ControllerMessage>>) -> Result<Response<Self::SyncStream>, Status> {
        let mut inbound = request.into_inner();
        let (out_tx, out_rx) = mpsc::channel::<Result<CenterMessage, Status>>(32);
        let (inner_tx, mut inner_rx) = mpsc::channel::<CenterMessage>(32);

        // 1. Wait for RegisterRequest (5s timeout)
        let first_msg = tokio::time::timeout(Duration::from_secs(5), inbound.message())
            .await
            .map_err(|_| Status::deadline_exceeded("Registration timeout: no RegisterRequest within 5s"))?
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::cancelled("Stream closed before RegisterRequest"))?;

        let register_req = match first_msg.payload {
            Some(CtrlPayload::Register(r)) => r,
            _ => return Err(Status::invalid_argument("First message must be RegisterRequest")),
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

        // 2. Register in registry + aggregator (aggregator drives online_controllers() in fan-out)
        self.aggregator
            .set_controller_info(&controller_id, register_req.clone());
        // Persist registration to SQLite (best-effort, isolated from the hot path).
        // Any failure here is logged and swallowed — we refuse to block fed-sync
        // registration on DB availability, since the controller is already live
        // in-memory and remains operational without persistence.
        if let Some(db) = &self.db {
            let db = db.clone();
            let cid = controller_id.clone();
            let cluster = register_req.cluster.clone();
            let env = register_req.env.clone();
            let tag = register_req.tag.clone();
            tokio::task::spawn_blocking(move || {
                if let Err(e) = db.upsert_controller(&cid, &cluster, &env, &tag, /*online=*/ true) {
                    tracing::warn!(
                        component = "fed_server",
                        controller_id = %cid,
                        error = %e,
                        "Failed to upsert controller on register"
                    );
                }
            });
        }
        self.registry.register(
            controller_id.clone(),
            register_req,
            inner_tx.clone(),
            session_id.clone(),
        );

        // Send RegisterAck
        let _ = inner_tx
            .send(CenterMessage {
                payload: Some(CenterPayload::RegisterAck(RegisterAck {
                    session_id: session_id.clone(),
                })),
            })
            .await;

        // Send FedWatchRequest for PluginMetaData
        let pm_cache = self.sync_client.plugin_metadata.get_or_create(&controller_id);
        let from_version = pm_cache.get_sync_version();
        let watch_request_id = Uuid::new_v4().to_string();

        let _ = inner_tx
            .send(CenterMessage {
                payload: Some(CenterPayload::WatchRequest(FedWatchRequest {
                    request_id: watch_request_id.clone(),
                    kind: "PluginMetaData".to_string(),
                    from_version,
                })),
            })
            .await;

        tracing::debug!(
            component = "fed_server",
            controller_id = %controller_id,
            request_id = %watch_request_id,
            from_version = from_version,
            "Sent FedWatchRequest for PluginMetaData"
        );

        let registry = self.registry.clone();
        let aggregator = self.aggregator.clone();
        let pending_lists = self.pending_lists.clone();
        let pending_commands = self.pending_commands.clone();
        let pending_proxies = self.pending_proxies.clone();
        let sync_client = self.sync_client.clone();
        let db_for_offline = self.db.clone();
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
                            payload: Some(CenterPayload::Ping(Ping { timestamp: now_ms })),
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
            let aggregator_for_offline = aggregator.clone();
            let cid = cid.clone();
            let sid = session_id.clone();
            let self_pending_proxies = pending_proxies.clone();
            let pm_cache = pm_cache.clone();
            let inner_tx = inner_tx.clone();
            let db_for_offline = db_for_offline.clone();
            async move {
                // Centralize the three disconnect cleanups (timeout / stream error / stream closed).
                // Each branch must propagate offline to all four state holders, which previously
                // diverged easily when new state was added; this closure keeps them in lockstep.
                let mark_offline_all = || {
                    registry.mark_offline(&cid, &sid);
                    aggregator_for_offline.mark_offline(&cid);
                    sync_client.plugin_metadata.mark_offline(&cid);
                    if let Some(db) = &db_for_offline {
                        let db = db.clone();
                        let cid_db = cid.clone();
                        tokio::task::spawn_blocking(move || {
                            if let Err(e) = db.mark_offline(&cid_db) {
                                tracing::warn!(
                                    component = "fed_server",
                                    controller_id = %cid_db,
                                    error = %e,
                                    "Failed to mark controller offline in DB"
                                );
                            }
                        });
                    }
                };

                // Watch state tracking (local to this session)
                let mut pm_watch = FedWatchState::new(watch_request_id, {
                    let sid = pm_cache.get_server_id();
                    if sid.is_empty() {
                        None
                    } else {
                        Some(sid)
                    }
                });

                loop {
                    match tokio::time::timeout(heartbeat_timeout, inbound.message()).await {
                        Err(_) => {
                            tracing::warn!(
                                component = "fed_server",
                                controller_id = %cid,
                                "Heartbeat timeout, marking offline"
                            );
                            mark_offline_all();
                            break;
                        }
                        Ok(Err(e)) => {
                            tracing::info!(
                                component = "fed_server",
                                controller_id = %cid,
                                error = %e,
                                "Stream error"
                            );
                            mark_offline_all();
                            break;
                        }
                        Ok(Ok(None)) => {
                            tracing::info!(
                                component = "fed_server",
                                controller_id = %cid,
                                "Stream closed"
                            );
                            mark_offline_all();
                            break;
                        }
                        Ok(Ok(Some(msg))) => {
                            registry.update_last_seen(&cid);
                            match msg.payload {
                                Some(CtrlPayload::Pong(_)) => {}
                                Some(CtrlPayload::ListResponse(resp)) => {
                                    if let Some(entry) = pending_lists.lock().remove(&resp.request_id) {
                                        let _ = entry.sender.send(resp.keys.clone());
                                    }
                                    aggregator.update_snapshot(&cid, resp.keys);
                                }
                                Some(CtrlPayload::CommandResponse(resp)) => {
                                    if let Some(sender) = pending_commands.lock().remove(&resp.request_id) {
                                        let _ = sender.send(resp);
                                    }
                                }
                                Some(CtrlPayload::HttpProxyResponse(resp)) => {
                                    if let Some(tx) = self_pending_proxies.lock().remove(&resp.request_id) {
                                        let _ = tx.send(resp);
                                    }
                                }
                                Some(CtrlPayload::WatchListResponse(resp)) => {
                                    if resp.request_id != pm_watch.request_id {
                                        tracing::debug!(
                                            component = "fed_server",
                                            controller_id = %cid,
                                            expected = %pm_watch.request_id,
                                            got = %resp.request_id,
                                            "Skipping stale WatchListResponse"
                                        );
                                        continue;
                                    }
                                    match serde_json::from_str::<Vec<PluginMetaData>>(&resp.data) {
                                        Ok(items) => {
                                            let keyed: Vec<(String, PluginMetaData)> = items
                                                .into_iter()
                                                .map(|pm| {
                                                    let key = pm.key_name();
                                                    (key, pm)
                                                })
                                                .collect();
                                            pm_cache.replace_all(keyed, resp.sync_version, resp.server_id.clone());
                                            pm_watch.server_id = Some(resp.server_id);
                                            pm_watch.consecutive_errors = 0;
                                            tracing::debug!(
                                                component = "fed_server",
                                                controller_id = %cid,
                                                sync_version = resp.sync_version,
                                                "PluginMetaData WatchListResponse applied"
                                            );
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                component = "fed_server",
                                                controller_id = %cid,
                                                error = %e,
                                                "Failed to deserialize WatchListResponse data"
                                            );
                                        }
                                    }
                                }
                                Some(CtrlPayload::WatchEventResponse(resp)) => {
                                    if resp.request_id != pm_watch.request_id {
                                        tracing::debug!(
                                            component = "fed_server",
                                            controller_id = %cid,
                                            expected = %pm_watch.request_id,
                                            got = %resp.request_id,
                                            "Skipping stale WatchEventResponse"
                                        );
                                        continue;
                                    }

                                    // Error → backoff then re-watch from 0
                                    if !resp.error.is_empty() {
                                        pm_watch.consecutive_errors += 1;
                                        if pm_watch.consecutive_errors == 1 {
                                            // First error is normal during startup/reload
                                            tracing::info!(
                                                component = "fed_server",
                                                controller_id = %cid,
                                                error = %resp.error,
                                                "WatchEventResponse error (likely startup delay), backing off before re-watch"
                                            );
                                        } else {
                                            tracing::warn!(
                                                component = "fed_server",
                                                controller_id = %cid,
                                                error = %resp.error,
                                                consecutive_errors = pm_watch.consecutive_errors,
                                                "WatchEventResponse error persists, backing off before re-watch"
                                            );
                                        }
                                        // Backoff before retrying to avoid tight loop.
                                        // Use select! to detect session close during sleep.
                                        tokio::select! {
                                            _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {}
                                            _ = inner_tx.closed() => {
                                                tracing::debug!(
                                                    component = "fed_server",
                                                    controller_id = %cid,
                                                    "Session closed during backoff, stopping re-watch"
                                                );
                                                break;
                                            }
                                        }
                                        let re_watch_msg = pm_watch.re_watch("PluginMetaData");
                                        let _ = inner_tx.send(re_watch_msg).await;
                                        continue;
                                    }

                                    // Server restart detection → re-watch from 0
                                    if let Some(ref expected_sid) = pm_watch.server_id {
                                        if *expected_sid != resp.server_id {
                                            tracing::warn!(
                                                component = "fed_server",
                                                controller_id = %cid,
                                                expected_server_id = %expected_sid,
                                                got_server_id = %resp.server_id,
                                                "Controller server_id changed, re-watching from 0"
                                            );
                                            let re_watch_msg = pm_watch.re_watch("PluginMetaData");
                                            let _ = inner_tx.send(re_watch_msg).await;
                                            continue;
                                        }
                                    }

                                    // Parse events using typed struct
                                    match serde_json::from_str::<Vec<WatchEventRaw>>(&resp.data) {
                                        Ok(raw_events) => {
                                            let mut events = Vec::new();
                                            for raw in raw_events {
                                                let event_type = match raw.event_type.as_str() {
                                                    "add" => EventType::Add,
                                                    "update" => EventType::Update,
                                                    "delete" => EventType::Delete,
                                                    other => {
                                                        tracing::warn!(
                                                            component = "fed_server",
                                                            controller_id = %cid,
                                                            event_type = other,
                                                            "Unknown watch event type, skipping"
                                                        );
                                                        continue;
                                                    }
                                                };
                                                match serde_json::from_value::<PluginMetaData>(raw.data) {
                                                    Ok(pm) => {
                                                        let key = pm.key_name();
                                                        events.push(WatchEventSimple {
                                                            event_type,
                                                            key,
                                                            data: pm,
                                                        });
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!(
                                                            component = "fed_server",
                                                            controller_id = %cid,
                                                            error = %e,
                                                            "Failed to parse watch event data as PluginMetaData"
                                                        );
                                                    }
                                                }
                                            }
                                            if !events.is_empty() {
                                                pm_cache.apply_events(
                                                    events,
                                                    resp.sync_version,
                                                    resp.server_id.clone(),
                                                );
                                            }
                                            pm_watch.server_id = Some(resp.server_id);
                                            pm_watch.consecutive_errors = 0;
                                            tracing::debug!(
                                                component = "fed_server",
                                                controller_id = %cid,
                                                sync_version = resp.sync_version,
                                                "PluginMetaData WatchEventResponse applied"
                                            );
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                component = "fed_server",
                                                controller_id = %cid,
                                                error = %e,
                                                "Failed to parse WatchEventResponse data"
                                            );
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(out_rx)))
    }
}
