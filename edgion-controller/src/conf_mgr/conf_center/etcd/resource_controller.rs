//! EtcdResourceController — per-kind event loop processing EtcdEvents.
//!
//! Mirrors `FileSystemResourceController` but:
//! - Receives `EtcdEvent` from bounded mpsc channel (backpressure, no event loss)
//! - Uses `EtcdStatusHandler` (async) for status persistence
//! - Worker reads from `LocalRawStore` (updated by event loop) instead of re-fetching from etcd

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use kube::Resource;
use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::task::JoinHandle;

use super::event::EtcdEvent;
use super::status::EtcdStatusHandler;
use super::watcher::KindEventReceiver;
use crate::conf_mgr::conf_center::common::parse_resource_content;
use crate::conf_mgr::sync_runtime::metrics::{controller_metrics, InitSyncTimer};
use crate::conf_mgr::sync_runtime::resource_processor::raw_store::RawStore;
use crate::conf_mgr::sync_runtime::resource_processor::{
    extract_status_value, make_resource_key, LocalRawStore, ResourceProcessor, WorkItemResult,
};
use crate::conf_mgr::sync_runtime::ShutdownSignal;
use edgion_resources::ResourceMeta;

type PendingIdentityErrors = RwLock<HashMap<String, String>>;

/// EtcdResourceController — handles events for a single resource type.
pub struct EtcdResourceController<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    kind: &'static str,
    processor: Arc<ResourceProcessor<K>>,
    status_handler: Arc<EtcdStatusHandler>,
    event_rx: KindEventReceiver,
    conf_prefix: Arc<str>,
    shutdown_signal: Option<ShutdownSignal>,
    fs_raw_store: Arc<LocalRawStore<K>>,
    pending_identity_errors: Arc<PendingIdentityErrors>,
}

impl<K> EtcdResourceController<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    pub fn new(
        kind: &'static str,
        processor: Arc<ResourceProcessor<K>>,
        status_handler: Arc<EtcdStatusHandler>,
        event_rx: KindEventReceiver,
        conf_prefix: Arc<str>,
    ) -> Self {
        let fs_raw_store = Arc::new(LocalRawStore::new());
        let raw_store: Arc<dyn RawStore<K>> = fs_raw_store.clone();
        processor.set_raw_store(raw_store.clone());
        crate::observe::status_metrics::register_status_renderer::<K>(kind, raw_store.clone());
        crate::observe::unknown_fields_registry::register_unknown_fields_source::<K>(kind, raw_store);
        Self {
            kind,
            processor,
            status_handler,
            event_rx,
            conf_prefix,
            shutdown_signal: None,
            fs_raw_store,
            pending_identity_errors: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn with_shutdown(mut self, signal: ShutdownSignal) -> Self {
        self.shutdown_signal = Some(signal);
        self
    }

    /// Run the event loop.
    pub async fn run(mut self) -> Result<()> {
        let kind = self.kind;

        controller_metrics().controller_started();
        tracing::info!(
            component = "etcd_resource_controller",
            kind = kind,
            "Starting EtcdResourceController"
        );

        let mut init_done = false;
        let mut init_count: usize = 0;
        let mut init_timer: Option<InitSyncTimer> = None;
        let mut worker_handle: Option<JoinHandle<()>> = None;
        let mut shutdown = self.shutdown_signal.clone();

        loop {
            let event = if let Some(ref mut signal) = shutdown {
                tokio::select! {
                    event = self.event_rx.recv() => event,
                    _ = signal.wait() => {
                        tracing::info!(
                            component = "etcd_resource_controller",
                            kind = kind,
                            "Shutdown signal received"
                        );
                        break;
                    }
                }
            } else {
                self.event_rx.recv().await
            };

            match event {
                Some(event) => match event {
                    EtcdEvent::Init => {
                        if !init_done {
                            tracing::debug!(
                                component = "etcd_resource_controller",
                                kind = kind,
                                "Init phase started"
                            );
                            init_timer = Some(InitSyncTimer::start(kind));
                            self.processor.on_init();
                        }
                    }
                    EtcdEvent::InitApply { key, content } => match parse_resource_content::<K>(&content) {
                        Ok(obj) => {
                            let content_key = make_resource_key(&obj);
                            let Some(rkey) =
                                validated_init_key(&self.status_handler, &self.conf_prefix, kind, &key, &content_key)
                                    .await
                            else {
                                continue;
                            };
                            self.fs_raw_store.upsert(rkey.clone(), Arc::new(obj.clone()));

                            let existing_status_json = self.status_handler.read_status_json(kind, &rkey).await;

                            let result = self.processor.on_init_apply(obj, existing_status_json).await;

                            if let WorkItemResult::Processed { obj, status_changed } = result {
                                init_count += 1;
                                if status_changed {
                                    self.persist_status(kind, &rkey, &obj).await;
                                }
                            }
                        }
                        Err(e) => {
                            let rkey = extract_key_from_etcd_key(&key, &self.conf_prefix, kind);
                            if let Some(rkey) = rkey {
                                let _ = self
                                    .status_handler
                                    .write_error_status(kind, &rkey, "ParseError", &e.to_string())
                                    .await;
                            }
                            tracing::warn!(
                                component = "etcd_resource_controller",
                                kind = kind,
                                key = %key,
                                error = %e,
                                "Failed to parse resource during init"
                            );
                        }
                    },
                    EtcdEvent::InitDone => {
                        let init_duration = init_timer.take().map(|t| t.complete(init_count)).unwrap_or(0.0);
                        tracing::info!(
                            component = "etcd_resource_controller",
                            kind = kind,
                            count = init_count,
                            duration_secs = init_duration,
                            "Init phase complete"
                        );

                        self.processor.on_init_done();
                        self.processor.notify_ready();
                        init_done = true;

                        worker_handle = Some(spawn_worker(
                            self.processor.clone(),
                            kind,
                            self.shutdown_signal.clone(),
                            self.fs_raw_store.clone(),
                            self.status_handler.clone(),
                            self.pending_identity_errors.clone(),
                        ));
                    }
                    EtcdEvent::Apply { key, content } => {
                        if !init_done {
                            // During init, treat as InitApply
                            if let Ok(obj) = parse_resource_content::<K>(&content) {
                                let content_key = make_resource_key(&obj);
                                let Some(rkey) = validated_init_key(
                                    &self.status_handler,
                                    &self.conf_prefix,
                                    kind,
                                    &key,
                                    &content_key,
                                )
                                .await
                                else {
                                    continue;
                                };
                                self.fs_raw_store.upsert(rkey.clone(), Arc::new(obj.clone()));
                                let existing_status_json = self.status_handler.read_status_json(kind, &rkey).await;
                                let result = self.processor.on_init_apply(obj, existing_status_json).await;
                                if let WorkItemResult::Processed { obj, status_changed } = result {
                                    init_count += 1;
                                    if status_changed {
                                        self.persist_status(kind, &rkey, &obj).await;
                                    }
                                }
                            }
                        } else {
                            match parse_resource_content::<K>(&content) {
                                Ok(obj) => {
                                    let content_key = make_resource_key(&obj);
                                    let rkey = match validate_etcd_identity(&key, &self.conf_prefix, kind, &content_key)
                                    {
                                        Ok(rkey) => rkey,
                                        Err(EtcdIdentityError::Mismatch { key_identity }) => {
                                            let message = identity_mismatch_message(&content_key, &key_identity);
                                            store_pending_identity_error(
                                                &self.pending_identity_errors,
                                                key_identity.clone(),
                                                message,
                                            );
                                            log_identity_mismatch(kind, &key, &content_key, &key_identity);
                                            self.processor.on_event_key(key_identity).await;
                                            continue;
                                        }
                                        Err(EtcdIdentityError::MalformedKey) => {
                                            log_malformed_etcd_key(kind, &key);
                                            continue;
                                        }
                                    };
                                    clear_pending_identity_error(&self.pending_identity_errors, &rkey);
                                    self.fs_raw_store.upsert(rkey.clone(), Arc::new(obj.clone()));
                                    self.processor.on_event_key(rkey).await;
                                }
                                Err(e) => {
                                    if let Some(rkey) = extract_key_from_etcd_key(&key, &self.conf_prefix, kind) {
                                        clear_pending_identity_error(&self.pending_identity_errors, &rkey);
                                        self.fs_raw_store.remove(&rkey);
                                    }
                                    tracing::warn!(
                                        component = "etcd_resource_controller",
                                        kind = kind,
                                        key = %key,
                                        error = %e,
                                        "Failed to parse resource for apply"
                                    );
                                }
                            }
                        }
                    }
                    EtcdEvent::Delete { key } => {
                        if init_done {
                            if let Some(rkey) = extract_key_from_etcd_key(&key, &self.conf_prefix, kind) {
                                clear_pending_identity_error(&self.pending_identity_errors, &rkey);
                                self.fs_raw_store.remove(&rkey);
                                self.processor.on_event_key(rkey).await;
                            } else {
                                log_malformed_etcd_key(kind, &key);
                            }
                        }
                    }
                },
                None => {
                    tracing::warn!(
                        component = "etcd_resource_controller",
                        kind = kind,
                        "Event channel closed"
                    );
                    break;
                }
            }
        }

        // Wait for worker
        if let Some(handle) = worker_handle {
            match tokio::time::timeout(Duration::from_secs(5), handle).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::error!(
                        component = "etcd_resource_controller",
                        kind = kind,
                        error = %e,
                        "Worker task panicked"
                    );
                }
                Err(_) => {
                    tracing::warn!(
                        component = "etcd_resource_controller",
                        kind = kind,
                        "Worker task did not finish within 5 seconds"
                    );
                }
            }
        }

        controller_metrics().controller_stopped();
        tracing::info!(
            component = "etcd_resource_controller",
            kind = kind,
            "EtcdResourceController stopped"
        );
        Ok(())
    }

    /// Persist status to etcd.
    async fn persist_status(&self, kind: &str, key: &str, obj: &K) {
        if let Some(status_value) = extract_status_value(obj) {
            match self.status_handler.write_status_value(kind, key, &status_value).await {
                Ok(()) => {
                    crate::conf_mgr::sync_runtime::record_status_write(kind, "etcd");
                }
                Err(e) => {
                    crate::conf_mgr::sync_runtime::record_status_write_error(kind, "etcd");
                    tracing::warn!(
                        component = "etcd_resource_controller",
                        kind = kind,
                        key = %key,
                        error = %e,
                        "Failed to write status to etcd"
                    );
                }
            }
        }
    }
}

/// Spawn worker task for processing workqueue items.
///
/// Unlike FileSystem mode which re-reads files, the etcd worker uses
/// the `LocalRawStore` which is always up-to-date from the event loop.
fn spawn_worker<K>(
    processor: Arc<ResourceProcessor<K>>,
    kind: &'static str,
    shutdown_signal: Option<ShutdownSignal>,
    fs_raw_store: Arc<LocalRawStore<K>>,
    status_handler: Arc<EtcdStatusHandler>,
    pending_identity_errors: Arc<PendingIdentityErrors>,
) -> JoinHandle<()>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + Serialize + DeserializeOwned + 'static,
{
    let workqueue = processor.workqueue();

    tokio::spawn(async move {
        let mut shutdown = shutdown_signal;

        loop {
            let item = match &mut shutdown {
                Some(signal) => {
                    tokio::select! {
                        item = workqueue.dequeue() => item,
                        _ = signal.wait() => break,
                    }
                }
                None => workqueue.dequeue().await,
            };

            match item {
                Some(work_item) => {
                    if let Some(message) = take_pending_identity_error(&pending_identity_errors, &work_item.key) {
                        if let Err(e) = status_handler
                            .write_error_status(kind, &work_item.key, "IdentityMismatch", &message)
                            .await
                        {
                            tracing::warn!(
                                component = "etcd_resource_controller",
                                kind = kind,
                                key = %work_item.key,
                                error = %e,
                                "Failed to write identity-mismatch error status"
                            );
                        }
                        workqueue.done(&work_item.key);
                        continue;
                    }

                    // Get the latest object from raw_store (updated by event loop)
                    let store_obj = fs_raw_store.get(&work_item.key).map(|arc_obj| (*arc_obj).clone());

                    // Status read only when there is an object to process — in
                    // etcd mode this is a network RPC; deletes must not pay it.
                    let existing_status_json = if store_obj.is_some() {
                        status_handler.read_status_json(kind, &work_item.key).await
                    } else {
                        None
                    };

                    let result = processor
                        .process_work_item(
                            &work_item.key,
                            store_obj,
                            existing_status_json,
                            work_item.trigger_chain.clone(),
                        )
                        .await;

                    match result {
                        WorkItemResult::Processed { obj, status_changed } => {
                            if status_changed {
                                if let Some(status_value) = extract_status_value(&obj) {
                                    match status_handler
                                        .write_status_value(kind, &work_item.key, &status_value)
                                        .await
                                    {
                                        Ok(()) => {
                                            crate::conf_mgr::sync_runtime::record_status_write(kind, "etcd");
                                        }
                                        Err(e) => {
                                            crate::conf_mgr::sync_runtime::record_status_write_error(kind, "etcd");
                                            tracing::warn!(
                                                component = "etcd_resource_controller",
                                                kind = kind,
                                                key = %work_item.key,
                                                error = %e,
                                                "Failed to write status to etcd"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        WorkItemResult::Deleted { key } => {
                            let _ = status_handler.delete_status(kind, &key).await;
                        }
                        WorkItemResult::Skipped => {}
                    }

                    workqueue.done(&work_item.key);
                }
                None => break,
            }
        }

        tracing::info!(component = "etcd_resource_controller", kind = kind, "Worker task ended");
    })
}

fn store_pending_identity_error(pending: &PendingIdentityErrors, key: String, message: String) {
    pending.write().insert(key, message);
}

fn clear_pending_identity_error(pending: &PendingIdentityErrors, key: &str) {
    pending.write().remove(key);
}

fn take_pending_identity_error(pending: &PendingIdentityErrors, key: &str) -> Option<String> {
    pending.write().remove(key)
}

/// Extract resource key ("namespace/name" or "name") from etcd key.
///
/// Key format: `{prefix}/conf/{kind}/{namespace}/{name}`
/// e.g. "/edgion/conf/HTTPRoute/default/my-route" → "default/my-route"
///      "/edgion/conf/GatewayClass/_/edgion"       → "edgion"
fn extract_key_from_etcd_key(etcd_key: &str, conf_prefix: &str, expected_kind: &str) -> Option<String> {
    let relative = etcd_key.strip_prefix(conf_prefix)?;
    let (kind, ns_and_name) = relative.split_once('/')?;
    if kind != expected_kind {
        return None;
    }
    let (ns, name) = ns_and_name.split_once('/')?;
    if ns.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    if ns == "_" {
        Some(name.to_string())
    } else {
        Some(format!("{}/{}", ns, name))
    }
}

#[derive(Debug, PartialEq, Eq)]
enum EtcdIdentityError {
    MalformedKey,
    Mismatch { key_identity: String },
}

/// Return the authoritative key identity only when the etcd key is strictly
/// formed and agrees with the resource content.
fn validate_etcd_identity(
    etcd_key: &str,
    conf_prefix: &str,
    expected_kind: &str,
    content_key: &str,
) -> Result<String, EtcdIdentityError> {
    let key_identity =
        extract_key_from_etcd_key(etcd_key, conf_prefix, expected_kind).ok_or(EtcdIdentityError::MalformedKey)?;
    if key_identity != content_key {
        return Err(EtcdIdentityError::Mismatch { key_identity });
    }
    Ok(key_identity)
}

fn identity_mismatch_message(content_key: &str, key_identity: &str) -> String {
    format!(
        "resource content identity '{content_key}' does not match etcd key identity '{key_identity}'; \
         in etcd mode the key is authoritative — move the value to the matching key or revert \
         metadata.name/namespace to '{key_identity}'"
    )
}

fn log_identity_mismatch(kind: &str, etcd_key: &str, content_key: &str, key_identity: &str) {
    tracing::warn!(
        component = "etcd_resource_controller",
        kind = kind,
        key = %etcd_key,
        content_key = %content_key,
        key_identity = %key_identity,
        "Rejected resource: content identity does not match etcd key identity"
    );
}

fn log_malformed_etcd_key(kind: &str, etcd_key: &str) {
    tracing::warn!(
        component = "etcd_resource_controller",
        kind = kind,
        key = %etcd_key,
        "Rejected resource: malformed etcd resource key"
    );
}

async fn validated_init_key(
    status_handler: &EtcdStatusHandler,
    conf_prefix: &str,
    kind: &str,
    etcd_key: &str,
    content_key: &str,
) -> Option<String> {
    match validate_etcd_identity(etcd_key, conf_prefix, kind, content_key) {
        Ok(key_identity) => Some(key_identity),
        Err(EtcdIdentityError::Mismatch { key_identity }) => {
            reject_identity_mismatch(status_handler, kind, etcd_key, content_key, &key_identity).await;
            None
        }
        Err(EtcdIdentityError::MalformedKey) => {
            log_malformed_etcd_key(kind, etcd_key);
            None
        }
    }
}

/// Reject content whose metadata identity disagrees with its authoritative
/// etcd key and persist the diagnostic under that key's identity.
async fn reject_identity_mismatch(
    status_handler: &EtcdStatusHandler,
    kind: &str,
    etcd_key: &str,
    content_key: &str,
    key_identity: &str,
) {
    let message = identity_mismatch_message(content_key, key_identity);
    if let Err(e) = status_handler
        .write_error_status(kind, key_identity, "IdentityMismatch", &message)
        .await
    {
        tracing::warn!(
            component = "etcd_resource_controller",
            kind = kind,
            key = %etcd_key,
            error = %e,
            "Failed to write identity-mismatch error status"
        );
    }
    log_identity_mismatch(kind, etcd_key, content_key, key_identity);
}

#[cfg(test)]
mod tests {
    use super::{
        clear_pending_identity_error, store_pending_identity_error, take_pending_identity_error,
        validate_etcd_identity, EtcdIdentityError, PendingIdentityErrors,
    };

    const CONF_PREFIX: &str = "/edgion/conf/";

    #[test]
    fn namespaced_identity_match_returns_key_identity() {
        assert_eq!(
            validate_etcd_identity(
                "/edgion/conf/HTTPRoute/default/r1",
                CONF_PREFIX,
                "HTTPRoute",
                "default/r1"
            ),
            Ok("default/r1".to_string())
        );
    }

    #[test]
    fn namespaced_identity_mismatch_returns_key_identity() {
        assert_eq!(
            validate_etcd_identity(
                "/edgion/conf/HTTPRoute/default/r1",
                CONF_PREFIX,
                "HTTPRoute",
                "other/r2"
            ),
            Err(EtcdIdentityError::Mismatch {
                key_identity: "default/r1".to_string()
            })
        );
    }

    #[test]
    fn cluster_scoped_identity_match_returns_key_identity() {
        assert_eq!(
            validate_etcd_identity("/edgion/conf/GatewayClass/_/gc1", CONF_PREFIX, "GatewayClass", "gc1"),
            Ok("gc1".to_string())
        );
    }

    #[test]
    fn empty_content_name_is_rejected() {
        assert_eq!(
            validate_etcd_identity(
                "/edgion/conf/HTTPRoute/default/r1",
                CONF_PREFIX,
                "HTTPRoute",
                "default/"
            ),
            Err(EtcdIdentityError::Mismatch {
                key_identity: "default/r1".to_string()
            })
        );
    }

    #[test]
    fn missing_name_in_etcd_key_is_rejected() {
        assert_eq!(
            validate_etcd_identity("/edgion/conf/HTTPRoute/default", CONF_PREFIX, "HTTPRoute", "default/r1"),
            Err(EtcdIdentityError::MalformedKey)
        );
    }

    #[test]
    fn empty_name_in_etcd_key_is_rejected() {
        assert_eq!(
            validate_etcd_identity("/edgion/conf/HTTPRoute/default/", CONF_PREFIX, "HTTPRoute", "default/"),
            Err(EtcdIdentityError::MalformedKey)
        );
    }

    #[test]
    fn extra_key_segment_is_rejected() {
        assert_eq!(
            validate_etcd_identity(
                "/edgion/conf/HTTPRoute/default/r1/extra",
                CONF_PREFIX,
                "HTTPRoute",
                "default/r1"
            ),
            Err(EtcdIdentityError::MalformedKey)
        );
    }

    #[test]
    fn key_prefix_may_contain_conf_segment() {
        assert_eq!(
            validate_etcd_identity(
                "/tenant/conf/edgion/conf/HTTPRoute/default/r1",
                "/tenant/conf/edgion/conf/",
                "HTTPRoute",
                "default/r1"
            ),
            Ok("default/r1".to_string())
        );
    }

    #[test]
    fn nested_backend_marker_after_conf_prefix_is_rejected() {
        assert_eq!(
            validate_etcd_identity(
                "/edgion/conf/HTTPRoute/junk/x/conf/HTTPRoute/default/r1",
                CONF_PREFIX,
                "HTTPRoute",
                "default/r1"
            ),
            Err(EtcdIdentityError::MalformedKey)
        );
    }

    #[test]
    fn valid_event_clears_pending_identity_error() {
        let pending = PendingIdentityErrors::default();
        store_pending_identity_error(&pending, "default/r1".to_string(), "mismatch".to_string());
        clear_pending_identity_error(&pending, "default/r1");
        assert_eq!(take_pending_identity_error(&pending, "default/r1"), None);
    }

    #[test]
    fn worker_takes_pending_identity_error_once() {
        let pending = PendingIdentityErrors::default();
        store_pending_identity_error(&pending, "default/r1".to_string(), "mismatch".to_string());
        assert_eq!(
            take_pending_identity_error(&pending, "default/r1"),
            Some("mismatch".to_string())
        );
        assert_eq!(take_pending_identity_error(&pending, "default/r1"), None);
    }
}
