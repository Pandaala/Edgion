use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use etcd_client::{Client, EventType, GetOptions, WatchOptions};
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::core::conf_load::{ConfigLoader, SharedDispatcher};
use crate::core::conf_sync::traits::{EventDispatcher, ResourceChange};
use crate::types::ResourceKind;

#[derive(Clone)]
pub struct EtcdConfigLoader {
    endpoints: Vec<String>,
    prefix: String,
    dispatcher: SharedDispatcher,
    resource_kind: Option<ResourceKind>,
    cache: Arc<Mutex<HashMap<String, String>>>,
}

impl EtcdConfigLoader {
    pub fn new(
        endpoints: Vec<String>,
        prefix: impl Into<String>,
        dispatcher: SharedDispatcher,
        resource_kind: Option<ResourceKind>,
    ) -> Arc<Self> {
        Arc::new(Self {
            endpoints,
            prefix: prefix.into(),
            dispatcher,
            resource_kind,
            cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn spawn(self: Arc<Self>) -> JoinHandle<()> {
        let prefix = self.prefix.clone();
        tokio::spawn(async move {
            if let Err(err) = self.run().await {
                eprintln!(
                    "[EtcdConfigLoader] watcher failed for prefix {}: {}",
                    prefix, err
                );
            }
        })
    }

    async fn dispatch_change(&self, change: ResourceChange, payload: String) {
        let dispatcher = &*self.dispatcher.lock().await;
        dispatcher.apply_resource_change(change, self.resource_kind, payload, None);
    }

    async fn bootstrap(&self, client: &mut Client) -> Result<()> {
        let options = GetOptions::new().with_prefix();
        let resp = client
            .get(self.prefix.clone(), Some(options))
            .await
            .with_context(|| format!("Failed to fetch initial keys for prefix {}", self.prefix))?;

        let mut cache_guard = self.cache.lock().await;
        cache_guard.clear();

        for kv in resp.kvs() {
            if let Ok(value) = String::from_utf8(kv.value().to_vec()) {
                let key = String::from_utf8_lossy(kv.key()).to_string();
                cache_guard.insert(key, value.clone());
                drop(cache_guard);
                self.dispatch_change(ResourceChange::EventAdd, value).await;
                cache_guard = self.cache.lock().await;
            }
        }

        drop(cache_guard);
        Ok(())
    }

    async fn handle_put(&self, key: String, value: String) {
        let mut cache = self.cache.lock().await;
        if let Some(old) = cache.remove(&key) {
            drop(cache);
            self.dispatch_change(ResourceChange::EventDelete, old).await;
            cache = self.cache.lock().await;
        }
        cache.insert(key, value.clone());
        drop(cache);
        self.dispatch_change(ResourceChange::EventAdd, value).await;
    }

    async fn handle_delete(&self, key: String) {
        let mut cache = self.cache.lock().await;
        if let Some(old) = cache.remove(&key) {
            drop(cache);
            self.dispatch_change(ResourceChange::EventDelete, old).await;
        }
    }
}

#[async_trait::async_trait]
impl ConfigLoader for EtcdConfigLoader {
    async fn run(self: Arc<Self>) -> Result<()> {
        if self.endpoints.is_empty() {
            return Err(anyhow!("No etcd endpoints provided"));
        }

        let mut client = Client::connect(self.endpoints.clone(), None)
            .await
            .context("Failed to connect to etcd")?;

        self.bootstrap(&mut client).await?;

        let watch_options = WatchOptions::new().with_prefix();
        let (_watcher, mut stream) = client
            .watch(self.prefix.clone(), Some(watch_options))
            .await
            .with_context(|| format!("Failed to watch prefix {}", self.prefix))?;

        while let Some(resp) = stream.next().await {
            let resp = resp?;
            for event in resp.events() {
                match event.event_type() {
                    EventType::Put => {
                        if let Some(kv) = event.kv() {
                            let key = String::from_utf8_lossy(kv.key()).to_string();
                            if let Ok(value) = String::from_utf8(kv.value().to_vec()) {
                                self.handle_put(key, value).await;
                            }
                        }
                    }
                    EventType::Delete => {
                        if let Some(kv) = event.kv() {
                            let key = String::from_utf8_lossy(kv.key()).to_string();
                            self.handle_delete(key).await;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
