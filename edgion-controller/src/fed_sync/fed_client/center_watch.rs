//! Dedicated Center view transport. This module never reads ConfigSyncServer.
use super::*;
use crate::conf_mgr::conf_center::traits::CenterLifeCycle;
use crate::fed_sync::center_watch::{
    projection::{Record, SourceKey},
    Change, StoreError,
};
use crate::fed_sync::proto::FedWatchRequest;
use serde::Serialize;
use tokio::sync::mpsc;

const VIEW: &str = "RegionRoute";
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// One bounded producer per federation session, including replacement/error tasks.
pub(super) struct WatchSession {
    active: Option<DropGuard>,
    pub(super) cancelled: CancellationToken,
    producer: Arc<Semaphore>,
}
impl Default for WatchSession {
    fn default() -> Self {
        Self {
            active: None,
            cancelled: CancellationToken::new(),
            producer: Arc::new(Semaphore::new(1)),
        }
    }
}
impl Drop for WatchSession {
    fn drop(&mut self) {
        self.cancelled.cancel();
    }
}
impl WatchSession {
    pub(super) fn start(
        &mut self,
        request: FedWatchRequest,
        tx: Arc<mpsc::Sender<ControllerMessage>>,
        manager: Arc<ConfMgr>,
        authorizer: Arc<Authorizer>,
    ) {
        // Drop untrusted large fields before spawning a queued task.
        let valid_id = !request.request_id.is_empty()
            && request.request_id.len() <= 128
            && !request.request_id.chars().any(char::is_control);
        let allowed_view = request.kind == VIEW;
        let from_version = request.from_version;
        let request_id = if valid_id { request.request_id } else { String::new() };
        drop(request.kind);
        drop(self.active.take());
        let cancel = self.cancelled.child_token();
        self.active = Some(cancel.clone().drop_guard());
        let session = self.cancelled.clone();
        let producer = self.producer.clone();
        tokio::spawn(async move {
            let permit = tokio::select! {
                biased;
                _ = cancel.cancelled() => return,
                result = tokio::time::timeout(SEND_TIMEOUT, producer.acquire_owned()) => result,
            };
            let Ok(Ok(_permit)) = permit else {
                session.cancel();
                return;
            };
            let context = Context {
                tx,
                manager,
                authorizer,
                cancel,
                session,
            };
            if !valid_id {
                context.error("", "InvalidRequest").await;
                return;
            }
            if !allowed_view || !context.authorized() {
                fed_metrics::record_rbac_denied(
                    "watch",
                    if allowed_view { VIEW } else { "unknown" },
                    fed_metrics::labels::rbac_source::CENTER,
                );
                context.error(&request_id, "Forbidden").await;
                return;
            }
            if from_version != 0 {
                context.error(&request_id, "RelistRequired").await;
                return;
            }
            if let Err(error) = context.run(&request_id).await {
                context.error(&request_id, error).await;
            }
        });
    }
}

struct Context {
    tx: Arc<mpsc::Sender<ControllerMessage>>,
    manager: Arc<ConfMgr>,
    authorizer: Arc<Authorizer>,
    cancel: CancellationToken,
    session: CancellationToken,
}
impl Context {
    fn authorized(&self) -> bool {
        self.authorizer
            .authorize(&self.authorizer.center_role(), Verb::Watch, VIEW)
            .allowed()
    }
    fn check(&self, generation: Option<&str>) -> Result<(), &'static str> {
        if self.cancel.is_cancelled() {
            return Err("Cancelled");
        }
        if !self.authorized() {
            return Err("Forbidden");
        }
        if !self.manager.is_ready() {
            return Err("NotReady");
        }
        if generation.is_some_and(|generation| self.manager.runtime().center_watch.generation() != generation) {
            return Err("RelistRequired");
        }
        Ok(())
    }
    /// Reserve before retaining snapshots or serializing. Failure closes the session:
    /// otherwise Center could keep a healthy mirror while only heartbeats continue.
    async fn reserve(&self) -> Option<mpsc::OwnedPermit<ControllerMessage>> {
        let result = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => return None,
            result = tokio::time::timeout(SEND_TIMEOUT, self.tx.as_ref().clone().reserve_owned()) => result,
        };
        match result {
            Ok(Ok(permit)) => Some(permit),
            _ => {
                self.session.cancel();
                None
            }
        }
    }
    async fn error(&self, request_id: &str, error: &'static str) {
        if let Some(permit) = self.reserve().await {
            if self.cancel.is_cancelled() {
                return;
            }
            permit.send(ControllerMessage {
                payload: Some(CtrlPayload::WatchEventResponse(FedWatchEventResponse {
                    request_id: request_id.into(),
                    error: error.into(),
                    ..Default::default()
                })),
            });
        }
    }
    async fn retry_failed(&self, generation: &str) {
        let runtime = self.manager.runtime();
        let Ok(keys) = runtime.center_watch.failed_keys(generation) else {
            return;
        };
        let retry = async {
            for key in keys {
                let kind = match key.kind {
                    crate::fed_sync::center_watch::projection::SourceKind::EdgionConfigData => "EdgionConfigData",
                    crate::fed_sync::center_watch::projection::SourceKind::EdgionPlugins => "EdgionPlugins",
                };
                tokio::select! {
                    biased;
                    _ = self.cancel.cancelled() => return,
                    _ = runtime.processor_registry.requeue(kind, format!("{}/{}", key.namespace, key.name)) => {},
                }
            }
        };
        let _ = tokio::time::timeout(SEND_TIMEOUT, retry).await;
    }
    async fn run(&self, request_id: &str) -> Result<(), &'static str> {
        self.check(None)?;
        let runtime = self.manager.runtime();
        // Subscribe before the snapshot. The version cursor covers changes occurring
        // while List is serialized or sent; notifications carry no resource bodies.
        let mut changed = runtime.center_watch.subscribe();
        let Some(permit) = self.reserve().await else {
            return Ok(());
        };
        self.check(None)?;
        let snapshot = match runtime.center_watch.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                drop(permit);
                self.retry_failed(&runtime.center_watch.generation()).await;
                return Err(store_error(error));
            }
        };
        let data = list_bytes(&snapshot.records)?;
        let generation = snapshot.generation;
        let mut version = snapshot.version;
        drop(snapshot.records);
        self.check(Some(&generation))?;
        permit.send(ControllerMessage {
            payload: Some(CtrlPayload::WatchListResponse(FedWatchListResponse {
                request_id: request_id.into(),
                data: data.into(),
                sync_version: version,
                server_id: generation.clone(),
            })),
        });
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => return Ok(()),
                result = changed.changed() => { if result.is_err() { return Err("RelistRequired"); } },
                _ = tick.tick() => {},
            }
            self.check(Some(&generation))?;
            // Inspect availability even on an idle tick, without retaining a snapshot.
            let (current, changes) = runtime
                .center_watch
                .events_since(&generation, version)
                .map_err(store_error)?;
            let has_changes = current != version;
            drop(changes);
            if !has_changes {
                continue;
            }
            let Some(permit) = self.reserve().await else {
                return Ok(());
            };
            self.check(Some(&generation))?;
            let (current, changes) = runtime
                .center_watch
                .events_since(&generation, version)
                .map_err(store_error)?;
            let data = event_bytes(&changes)?;
            drop(changes);
            self.check(Some(&generation))?;
            permit.send(ControllerMessage {
                payload: Some(CtrlPayload::WatchEventResponse(FedWatchEventResponse {
                    request_id: request_id.into(),
                    data: String::from_utf8(data).map_err(|_| "SerializationFailed")?,
                    sync_version: current,
                    server_id: generation.clone(),
                    error: String::new(),
                })),
            });
            version = current;
            fed_metrics::record_watch_event(VIEW, fed_metrics::labels::direction::SENT);
        }
    }
}
fn store_error(error: StoreError) -> &'static str {
    match error {
        StoreError::StaleGeneration | StoreError::TooOld | StoreError::FutureVersion => "RelistRequired",
        StoreError::Projection | StoreError::Capacity => "ViewUnavailable",
    }
}
fn encode(value: &impl Serialize) -> Result<Vec<u8>, &'static str> {
    // Typed store accounting already bounds retained live/history data to 4 MiB,
    // including worst-case JSON escaping. This 8 MiB cap includes the wire wrapper.
    let bytes = serde_json::to_vec(value).map_err(|_| "SerializationFailed")?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err("ViewUnavailable");
    }
    Ok(bytes)
}
fn list_bytes(records: &[Arc<Record>]) -> Result<Vec<u8>, &'static str> {
    #[derive(Serialize)]
    struct List<'a> {
        records: Vec<&'a Record>,
    }
    encode(&List {
        records: records.iter().map(Arc::as_ref).collect(),
    })
}
fn event_bytes(changes: &[Change]) -> Result<Vec<u8>, &'static str> {
    #[derive(Serialize)]
    struct Event<'a> {
        version: u64,
        key: &'a SourceKey,
        record: Option<&'a Record>,
    }
    #[derive(Serialize)]
    struct Events<'a> {
        changes: Vec<Event<'a>>,
    }
    encode(&Events {
        changes: changes
            .iter()
            .map(|change| Event {
                version: change.version,
                key: &change.key,
                record: change.record.as_deref(),
            })
            .collect(),
    })
}

#[cfg(test)]
#[path = "../../../tests/unit/fed_sync/fed_client/center_watch_test.rs"]
mod tests;
