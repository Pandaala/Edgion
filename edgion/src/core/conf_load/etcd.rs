use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use etcd_client::{Client, EventType, GetOptions, WatchOptions};
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::core::conf_load::{ConfigLoader};
use crate::core::conf_sync::traits::{EventDispatcher, ResourceChange};
use crate::core::utils::is_base_conf;
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

    async fn dispatch_change(&self, change: ResourceChange, payload: String, use_base_conf: bool) {
        if use_base_conf {
            self.dispatcher.apply_base_conf(change, self.resource_kind, payload, None);
        } else {
            self.dispatcher.apply_resource_change(change, self.resource_kind, payload, None);
        }
    }


    async fn handle_put(&self, key: String, value: String) {
        let use_base_conf = is_base_conf(&value);
        let mut cache = self.cache.lock().await;
        if let Some(old) = cache.remove(&key) {
            drop(cache);
            self.dispatch_change(ResourceChange::EventDelete, old, use_base_conf).await;
            cache = self.cache.lock().await;
        }
        cache.insert(key, value.clone());
        drop(cache);
        self.dispatch_change(ResourceChange::EventAdd, value, use_base_conf).await;
    }

    async fn handle_delete(&self, key: String) {
        let mut cache = self.cache.lock().await;
        if let Some(old) = cache.remove(&key) {
            let use_base_conf = is_base_conf(&old);
            drop(cache);
            self.dispatch_change(ResourceChange::EventDelete, old, use_base_conf).await;
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

    /// Bootstrap and load base configuration resources (GatewayClass, EdgionGatewayConfig, Gateway)
    /// If kind is specified, only load resources of that kind
    async fn bootstrap_base_conf(&self, kind: Option<crate::types::ResourceKind>) -> Result<()> {
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

        for kv in resp.kvs() {
            if let Ok(value) = String::from_utf8(kv.value().to_vec()) {
                if is_base_conf(&value) {
                    // Check kind filter if specified
                    if let Some(target_kind) = kind {
                        if let Some(content_kind) = ResourceKind::from_content(&value) {
                            if content_kind != target_kind {
                                continue;
                            }
                            
                            // For EdgionGatewayConfig, check if it's referenced by GatewayClass
                            if content_kind == ResourceKind::EdgionGatewayConfig {
                                // Parse the config name from content
                                if let Ok(config) = serde_yaml::from_str::<serde_yaml::Value>(&value) {
                                    if let Some(name) = config.get("metadata")
                                        .and_then(|m| m.get("name"))
                                        .and_then(|n| n.as_str())
                                    {
                                        if !self.dispatcher.should_load_edgion_gateway_config(name) {
                                            tracing::debug!(
                                                component = "etcd_loader",
                                                event = "skip_config_not_referenced",
                                                key = ?kv.key(),
                                                config_name = name,
                                                "Skipping EdgionGatewayConfig not referenced by GatewayClass parametersRef"
                                            );
                                            continue;
                                        }
                                    }
                                }
                            }
                        } else {
                            continue;
                        }
                    }
                    
                    let key = String::from_utf8_lossy(kv.key()).to_string();
                    cache_guard.insert(key, value.clone());
                    drop(cache_guard);
                    // Use InitAdd for bootstrap phase
                    self.dispatch_change(ResourceChange::InitAdd, value, true).await;
                    cache_guard = self.cache.lock().await;
                }
            }
        }

        drop(cache_guard);
        Ok(())
    }

    /// Bootstrap and load user configuration resources (all other resources)
    async fn bootstrap_user_conf(&self) -> Result<()> {
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

        for kv in resp.kvs() {
            if let Ok(value) = String::from_utf8(kv.value().to_vec()) {
                if !is_base_conf(&value) {
                    let key = String::from_utf8_lossy(kv.key()).to_string();
                    cache_guard.insert(key, value.clone());
                    drop(cache_guard);
                    // Use InitAdd for bootstrap phase
                    self.dispatch_change(ResourceChange::InitAdd, value, false).await;
                    cache_guard = self.cache.lock().await;
                }
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
