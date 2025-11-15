use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use etcd_client::{Client, EventType, GetOptions, WatchOptions};
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::core::conf_load::{ConfigLoader};
use crate::core::conf_sync::traits::{EventDispatcher, ResourceChange};
use crate::types::ResourceKind;

#[derive(Clone)]
pub struct EtcdConfigLoader {
    endpoints: Vec<String>,
    prefix: String,
    dispatcher: Arc<dyn EventDispatcher>,
    resource_kind: Option<ResourceKind>,
    cache: Arc<Mutex<HashMap<String, String>>>,
    client: Arc<Mutex<Option<Client>>>,
}

impl EtcdConfigLoader {
    pub fn new(
        endpoints: Vec<String>,
        prefix: impl Into<String>,
        dispatcher: Arc<dyn EventDispatcher>,
        resource_kind: Option<ResourceKind>,
    ) -> Arc<Self> {
        Arc::new(Self {
            endpoints,
            prefix: prefix.into(),
            dispatcher,
            resource_kind,
            cache: Arc::new(Mutex::new(HashMap::new())),
            client: Arc::new(Mutex::new(None)),
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
        self.dispatcher.apply_resource_change(change, self.resource_kind, payload, None);
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
    /// Connect to etcd cluster
    async fn connect(&self) -> Result<()> {
        if self.endpoints.is_empty() {
            return Err(anyhow!("No etcd endpoints provided"));
        }

        let client = Client::connect(self.endpoints.clone(), None)
            .await
            .context("Failed to connect to etcd")?;

        let mut client_guard = self.client.lock().await;
        *client_guard = Some(client);
        Ok(())
    }

    /// Bootstrap and load all existing configurations from etcd
    async fn bootstrap_existing(&self) -> Result<()> {
        let mut client_guard = self.client.lock().await;
        let client = client_guard
            .as_mut()
            .ok_or_else(|| anyhow!("Client not connected"))?;

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
                // Use InitAdd for bootstrap phase
                self.dispatch_change(ResourceChange::InitAdd, value).await;
                cache_guard = self.cache.lock().await;
            }
        }

        drop(cache_guard);
        Ok(())
    }

    /// Set ready state after initialization
    async fn set_ready(&self) {
        self.dispatcher.set_ready();
    }

    /// Main run loop for watching etcd configuration changes
    async fn run(&self) -> Result<()> {

        // Start watching for changes
        let mut client_guard = self.client.lock().await;
        let client = client_guard
            .as_mut()
            .ok_or_else(|| anyhow!("Client not connected"))?;

        let watch_options = WatchOptions::new().with_prefix();
        let (_watcher, mut stream) = client
            .watch(self.prefix.clone(), Some(watch_options))
            .await
            .with_context(|| format!("Failed to watch prefix {}", self.prefix))?;

        drop(client_guard);

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
