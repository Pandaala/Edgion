//! gRPC server implementing FederationSync.
//!
//! On each connection:
//! 1. Wait up to 5s for first ControllerMessage (must be RegisterRequest)
//! 2. Register controller in registry
//! 3. Spawn heartbeat task (Ping every ping_interval)
//! 4. Loop: forward incoming messages to aggregator/commander; forward outgoing to stream

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
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
    RegisterRequest,
};
use crate::core::common::observe::fed_metrics;
use crate::types::resource::meta::ResourceMeta;
use crate::types::resources::plugin_metadata::PluginMetaData;

/// Label value used on fed-sync metrics whose `kind` dimension is
/// currently hardcoded. The federation server only streams PluginMetaData
/// today; when new kinds are added, each call site should pass its own
/// kind instead of this constant.
const PLUGIN_METADATA_KIND: &str = "PluginMetaData";

// Boundary caps on RegisterRequest fields. The federation gRPC server is
// reachable in plaintext on default deployments; any peer that completes a
// TCP connect can submit a RegisterRequest, so each field that becomes a
// HashMap key, an SQLite row, or an Admin API payload must be bounded
// here. Caps follow K8s label-value conventions (63 bytes per token, 253
// bytes for full DNS-style identifiers) so legitimate controllers built
// from `cluster + name` are well below the limit.
const MAX_CONTROLLER_ID_LEN: usize = 253;
const MAX_CLUSTER_LEN: usize = 63;
const MAX_TAG_LEN: usize = 63;
const MAX_LIST_ITEMS: usize = 32;

/// Number of consecutive missed PINGs (no Pong received) before declaring the peer offline.
/// Three intervals allow one transient packet loss without spurious offline marking.
/// Per RFC 9113 §6.7 (informative).
const HEARTBEAT_MISSED_PING_BUDGET: u32 = 3;

// Upper bound on total registered controllers (online + offline). Once
// this is reached, registration of a *new* controller_id is refused;
// reconnects from already-known ids still go through. Default is sized
// well above realistic enterprise federation deployments (typically tens
// to a few hundred controllers) but below the point where an attacker can
// inflate registry / aggregator / SQLite to dangerous sizes.
const MAX_REGISTRY_ENTRIES: usize = 10_000;

/// Reject a `RegisterRequest` whose fields are empty, oversized, or
/// contain control characters.
///
/// Returning a static `&'static str` keeps the rejection reason out of
/// the response body — the caller emits the reason via `tracing::warn!`
/// for ops, and surfaces a fixed `Status::invalid_argument` message to
/// the peer so we never echo attacker-controlled bytes back.
fn validate_register_req(req: &RegisterRequest) -> Result<(), &'static str> {
    if req.controller_id.is_empty() {
        return Err("controller_id is empty");
    }
    if req.controller_id.len() > MAX_CONTROLLER_ID_LEN {
        return Err("controller_id exceeds max length");
    }
    if req.controller_id.chars().any(|c| c.is_control()) {
        return Err("controller_id contains control characters");
    }
    if req.cluster.len() > MAX_CLUSTER_LEN {
        return Err("cluster exceeds max length");
    }
    if req.cluster.chars().any(|c| c.is_control()) {
        return Err("cluster contains control characters");
    }
    validate_string_list(&req.env, "env")?;
    validate_string_list(&req.tag, "tag")?;
    validate_string_list(&req.supported_kinds, "supported_kinds")?;
    Ok(())
}

fn validate_string_list(items: &[String], field: &'static str) -> Result<(), &'static str> {
    if items.len() > MAX_LIST_ITEMS {
        return Err(match field {
            "env" => "env list exceeds max items",
            "tag" => "tag list exceeds max items",
            "supported_kinds" => "supported_kinds list exceeds max items",
            _ => "list exceeds max items",
        });
    }
    for item in items {
        if item.len() > MAX_TAG_LEN {
            return Err(match field {
                "env" => "env item exceeds max length",
                "tag" => "tag item exceeds max length",
                "supported_kinds" => "supported_kinds item exceeds max length",
                _ => "list item exceeds max length",
            });
        }
        if item.chars().any(|c| c.is_control()) {
            return Err(match field {
                "env" => "env item contains control characters",
                "tag" => "tag item contains control characters",
                "supported_kinds" => "supported_kinds item contains control characters",
                _ => "list item contains control characters",
            });
        }
    }
    Ok(())
}

/// True when the registry is at capacity *and* the incoming id is not
/// already known. Reconnect of a known controller is always allowed —
/// only inflation by new ids is refused, which preserves operator
/// recovery during a flood while still blocking unbounded growth.
fn registry_capacity_exceeded(registry: &ControllerRegistry, incoming_id: &str, cap: usize) -> bool {
    registry.len() >= cap && registry.get_session(incoming_id).is_none()
}

/// Typed representation of a single watch event from the controller.
///
/// `data` is borrowed as `&RawValue` so the outer batch parse only slices the
/// array; each item's payload is deserialized lazily into its concrete type,
/// avoiding an intermediate `serde_json::Value` tree per event.
#[derive(serde::Deserialize)]
struct WatchEventRaw<'a> {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(borrow)]
    data: &'a serde_json::value::RawValue,
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

pub type PendingCommandMap =
    Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<crate::core::common::fed_sync::proto::CommandResponse>>>>;

pub struct FederationGrpcServer {
    pub registry: ControllerRegistry,
    pub aggregator: Arc<ResourceAggregator>,
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

        // Boundary checks must run before any state mutation (registry,
        // aggregator, watch_cache, SQLite) so that a rejected request
        // leaves zero residue. Reasons are logged at warn level but the
        // peer-facing message is fixed to avoid echoing attacker input.
        if let Err(reason) = validate_register_req(&register_req) {
            tracing::warn!(
                component = "fed_server",
                reason = reason,
                controller_id_len = register_req.controller_id.len(),
                cluster_len = register_req.cluster.len(),
                env_len = register_req.env.len(),
                tag_len = register_req.tag.len(),
                supported_kinds_len = register_req.supported_kinds.len(),
                "Rejected RegisterRequest: shape validation failed"
            );
            return Err(Status::invalid_argument("RegisterRequest validation failed"));
        }
        if registry_capacity_exceeded(&self.registry, &register_req.controller_id, MAX_REGISTRY_ENTRIES) {
            tracing::warn!(
                component = "fed_server",
                registry_len = self.registry.len(),
                cap = MAX_REGISTRY_ENTRIES,
                "Rejected RegisterRequest: registry at capacity"
            );
            return Err(Status::resource_exhausted("Federation registry is at capacity"));
        }

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
                if let Err(e) = db.upsert_controller(&cid, &cluster, &env, &tag, true) {
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

        // Federation connection metrics: record the connect event and refresh
        // the active-sessions gauge. `online_len` captures the number of
        // controllers with a live `stream_tx`, so it naturally drops on
        // mark_offline without us counting in two places.
        fed_metrics::record_connection_event(fed_metrics::labels::role::CENTER, fed_metrics::labels::event::CONNECTED);
        fed_metrics::set_connections_active(fed_metrics::labels::role::CENTER, self.registry.online_len() as u64);
        let session_started_at = std::time::Instant::now();

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
                    kind: PLUGIN_METADATA_KIND.to_string(),
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
        let pending_commands = self.pending_commands.clone();
        let pending_proxies = self.pending_proxies.clone();
        let sync_client = self.sync_client.clone();
        let db_for_offline = self.db.clone();
        let ping_interval = Duration::from_secs(self.sync_config.ping_interval_secs);
        let heartbeat_timeout = ping_interval * HEARTBEAT_MISSED_PING_BUDGET;
        // Tracks the epoch-ms timestamp of the last received Pong. The heartbeat task reads
        // this to detect idle connections without wrapping message delivery in a timeout,
        // which would falsely fire on large in-flight WatchListResponse payloads.
        let last_pong_ms = Arc::new(AtomicU64::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        ));
        let heartbeat_cancel = CancellationToken::new();
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
            let last_pong_ms = last_pong_ms.clone();
            let heartbeat_cancel = heartbeat_cancel.clone();
            let heartbeat_timeout_ms = heartbeat_timeout.as_millis() as u64;
            async move {
                let mut interval = tokio::time::interval(ping_interval);
                interval.tick().await; // skip first
                loop {
                    interval.tick().await;
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    // Check Pong freshness before sending next Ping. Tracking last-pong-at
                    // rather than wrapping inbound.message() in a timeout avoids false offline
                    // declarations when a large WatchListResponse is in transit (RFC 9113 §6.7).
                    if now_ms.saturating_sub(last_pong_ms.load(Ordering::Relaxed)) > heartbeat_timeout_ms {
                        tracing::warn!(
                            component = "fed_server",
                            controller_id = %cid,
                            "Heartbeat timeout, marking offline"
                        );
                        heartbeat_cancel.cancel();
                        break;
                    }
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
            let aggregator_for_stats = aggregator.clone();
            let cid = cid.clone();
            let sid = session_id.clone();
            let self_pending_proxies = pending_proxies.clone();
            let pm_cache = pm_cache.clone();
            let inner_tx = inner_tx.clone();
            let db_for_offline = db_for_offline.clone();
            let last_pong_ms = last_pong_ms.clone();
            let heartbeat_cancel = heartbeat_cancel.clone();
            async move {
                // Centralize the three disconnect cleanups (timeout / stream error / stream closed).
                // Each branch must propagate offline to all four state holders, which previously
                // diverged easily when new state was added; this closure keeps them in lockstep.
                // The `reason` carries through to `edgion_fed_mark_offline_total{reason}` and
                // gates metric emission so repeated calls (e.g., race between heartbeat timeout
                // and stream close) do not double-count.
                let mark_offline_all = |reason: &'static str| {
                    let transitioned = registry.mark_offline(&cid, &sid);
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
                    if transitioned {
                        fed_metrics::record_mark_offline(reason);
                        fed_metrics::record_connection_event(
                            fed_metrics::labels::role::CENTER,
                            fed_metrics::labels::event::DISCONNECTED,
                        );
                        fed_metrics::record_connection_duration(
                            fed_metrics::labels::role::CENTER,
                            std::time::Instant::now()
                                .saturating_duration_since(session_started_at)
                                .as_secs_f64(),
                        );
                        fed_metrics::set_connections_active(
                            fed_metrics::labels::role::CENTER,
                            registry.online_len() as u64,
                        );
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
                    tokio::select! {
                        // Heartbeat task detected Pong silence > heartbeat_timeout.
                        _ = heartbeat_cancel.cancelled() => {
                            mark_offline_all(fed_metrics::labels::offline_reason::HEARTBEAT);
                            break;
                        }
                        result = inbound.message() => {
                            match result {
                            Err(e) => {
                                tracing::info!(
                                    component = "fed_server",
                                    controller_id = %cid,
                                    error = %e,
                                    "Stream error"
                                );
                                mark_offline_all(fed_metrics::labels::offline_reason::DISCONNECT);
                                break;
                            }
                            Ok(None) => {
                                tracing::info!(
                                    component = "fed_server",
                                    controller_id = %cid,
                                    "Stream closed"
                                );
                                mark_offline_all(fed_metrics::labels::offline_reason::DISCONNECT);
                                break;
                            }
                            Ok(Some(msg)) => {
                                registry.update_last_seen(&cid);
                                match msg.payload {
                                    Some(CtrlPayload::Pong(_)) => {
                                        last_pong_ms.store(
                                            std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .map(|d| d.as_millis() as u64)
                                                .unwrap_or(0),
                                            Ordering::Relaxed,
                                        );
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
                                            fed_metrics::record_watch_list(
                                                PLUGIN_METADATA_KIND,
                                                fed_metrics::labels::watch_list_result::OK,
                                            );
                                            tracing::debug!(
                                                component = "fed_server",
                                                controller_id = %cid,
                                                sync_version = resp.sync_version,
                                                "PluginMetaData WatchListResponse applied"
                                            );
                                        }
                                        Err(e) => {
                                            fed_metrics::record_watch_list(
                                                PLUGIN_METADATA_KIND,
                                                fed_metrics::labels::watch_list_result::PARSE_ERROR,
                                            );
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

                                    // Watch event delivered to Center (direction = recv from the
                                    // Center's point of view: events flow Controller → Center).
                                    fed_metrics::record_watch_event(
                                        PLUGIN_METADATA_KIND,
                                        fed_metrics::labels::direction::RECV,
                                    );

                                    // Error → backoff then re-watch from 0
                                    if !resp.error.is_empty() {
                                        fed_metrics::record_watch_error(
                                            PLUGIN_METADATA_KIND,
                                            fed_metrics::labels::watch_error_reason::RECV_ERROR,
                                        );
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
                                        let re_watch_msg = pm_watch.re_watch(PLUGIN_METADATA_KIND);
                                        let _ = inner_tx.send(re_watch_msg).await;
                                        continue;
                                    }

                                    // Server restart detection → re-watch from 0
                                    if let Some(ref expected_sid) = pm_watch.server_id {
                                        if *expected_sid != resp.server_id {
                                            fed_metrics::record_watch_list(
                                                PLUGIN_METADATA_KIND,
                                                fed_metrics::labels::watch_list_result::VERSION_TOO_OLD,
                                            );
                                            tracing::warn!(
                                                component = "fed_server",
                                                controller_id = %cid,
                                                expected_server_id = %expected_sid,
                                                got_server_id = %resp.server_id,
                                                "Controller server_id changed, re-watching from 0"
                                            );
                                            let re_watch_msg = pm_watch.re_watch(PLUGIN_METADATA_KIND);
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
                                                match serde_json::from_str::<PluginMetaData>(raw.data.get()) {
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
                                            fed_metrics::record_watch_error(
                                                PLUGIN_METADATA_KIND,
                                                fed_metrics::labels::watch_error_reason::PARSE_ERROR,
                                            );
                                            tracing::warn!(
                                                component = "fed_server",
                                                controller_id = %cid,
                                                error = %e,
                                                "Failed to parse WatchEventResponse data"
                                            );
                                        }
                                    }
                                }
                                Some(CtrlPayload::StatsReport(report)) => {
                                    // Push from Controller summarising per-kind resource counts.
                                    // Stored in aggregator and exposed via the API layer.
                                    aggregator_for_stats.update_stats(&cid, report.per_kind, report.total as u64);
                                }
                                _ => {}
                            }
                        }
                        }  // match result
                    }      // result = inbound.message() => { ... }
                    } // tokio::select!
                } // loop
            } // async move
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(out_rx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_req() -> RegisterRequest {
        RegisterRequest {
            controller_id: "cluster-a/ctrl-01".to_string(),
            cluster: "cluster-a".to_string(),
            env: vec!["prod".to_string()],
            tag: vec!["region:us".to_string()],
            supported_kinds: vec!["PluginMetaData".to_string()],
        }
    }

    #[test]
    fn validate_accepts_well_formed_request() {
        assert!(validate_register_req(&ok_req()).is_ok());
    }

    #[test]
    fn validate_rejects_empty_controller_id() {
        let mut r = ok_req();
        r.controller_id = String::new();
        assert!(validate_register_req(&r).is_err());
    }

    #[test]
    fn validate_rejects_overlong_controller_id() {
        let mut r = ok_req();
        r.controller_id = "x".repeat(MAX_CONTROLLER_ID_LEN + 1);
        assert!(validate_register_req(&r).is_err());
    }

    #[test]
    fn validate_rejects_controller_id_with_control_char() {
        let mut r = ok_req();
        r.controller_id = "ctrl\n01".to_string();
        assert!(validate_register_req(&r).is_err());
    }

    #[test]
    fn validate_allows_empty_cluster() {
        // Aggregator already normalises empty cluster to "unknown"; do not
        // tighten this without auditing aggregator gauge semantics.
        let mut r = ok_req();
        r.cluster = String::new();
        assert!(validate_register_req(&r).is_ok());
    }

    #[test]
    fn validate_rejects_overlong_cluster() {
        let mut r = ok_req();
        r.cluster = "y".repeat(MAX_CLUSTER_LEN + 1);
        assert!(validate_register_req(&r).is_err());
    }

    #[test]
    fn validate_rejects_cluster_with_control_char() {
        let mut r = ok_req();
        r.cluster = "cluster\u{0}a".to_string();
        assert!(validate_register_req(&r).is_err());
    }

    #[test]
    fn validate_rejects_too_many_env_items() {
        let mut r = ok_req();
        r.env = (0..(MAX_LIST_ITEMS + 1)).map(|i| format!("e{i}")).collect();
        assert!(validate_register_req(&r).is_err());
    }

    #[test]
    fn validate_rejects_overlong_env_item() {
        let mut r = ok_req();
        r.env = vec!["z".repeat(MAX_TAG_LEN + 1)];
        assert!(validate_register_req(&r).is_err());
    }

    #[test]
    fn validate_rejects_env_item_with_control_char() {
        let mut r = ok_req();
        r.env = vec!["prod\tprime".to_string()];
        assert!(validate_register_req(&r).is_err());
    }

    #[test]
    fn validate_rejects_overlong_tag_item() {
        let mut r = ok_req();
        r.tag = vec!["t".repeat(MAX_TAG_LEN + 1)];
        assert!(validate_register_req(&r).is_err());
    }

    #[test]
    fn validate_rejects_too_many_supported_kinds() {
        let mut r = ok_req();
        r.supported_kinds = (0..(MAX_LIST_ITEMS + 1)).map(|i| format!("K{i}")).collect();
        assert!(validate_register_req(&r).is_err());
    }

    #[test]
    fn capacity_gate_allows_under_cap() {
        let reg = ControllerRegistry::new();
        assert!(!registry_capacity_exceeded(&reg, "new-id", 4));
    }

    #[test]
    fn capacity_gate_refuses_new_id_at_cap() {
        let reg = ControllerRegistry::new();
        for i in 0..4 {
            let (tx, _rx) = tokio::sync::mpsc::channel(1);
            reg.register(format!("c{i}"), ok_req(), tx, format!("s{i}"));
        }
        assert!(registry_capacity_exceeded(&reg, "brand-new", 4));
    }

    #[test]
    fn capacity_gate_allows_known_id_at_cap() {
        // Reconnects must continue to work even when the registry is full.
        let reg = ControllerRegistry::new();
        for i in 0..4 {
            let (tx, _rx) = tokio::sync::mpsc::channel(1);
            reg.register(format!("c{i}"), ok_req(), tx, format!("s{i}"));
        }
        assert!(!registry_capacity_exceeded(&reg, "c0", 4));
    }
}
