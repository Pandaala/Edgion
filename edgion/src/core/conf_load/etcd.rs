use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use etcd_client::{Client, EventType, GetOptions, WatchOptions};
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::core::conf_load::ConfigLoader;
use crate::core::conf_sync::traits::{ConfigServerEventDispatcher, ResourceChange};
use crate::types::ResourceKind;

#[derive(Clone)]
pub struct EtcdConfigLoader {
    endpoints: Vec<String>,
    prefix: String,
    dispatcher: Arc<dyn ConfigServerEventDispatcher>,
    resource_kind: Option<ResourceKind>,
    cache: Arc<Mutex<HashMap<String, String>>>,
    client: Arc<Mutex<Option<Client>>>,
}

impl EtcdConfigLoader {
    pub fn new(
        endpoints: Vec<String>,
        prefix: impl Into<String>,
        dispatcher: Arc<dyn ConfigServerEventDispatcher>,
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
                eprintln!("[EtcdConfigLoader] watcher failed for prefix {}: {}", prefix, err);
            }
        })
    }

    async fn handle_put(&self, key: String, value: String) {
        // Determine if this is a base conf resource
        // Base conf resources (GatewayClass, EdgionGatewayConfig, Gateway) are loaded via load_base, not through watch
        let is_base_conf = if let Some(kind) = ResourceKind::from_content(&value) {
            matches!(
                kind,
                ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway
            )
        } else {
            false
        };

        // Skip base conf resources as they are handled by load_base
        if is_base_conf {
            return;
        }

        let mut cache = self.cache.lock().await;
        if let Some(old) = cache.remove(&key) {
            drop(cache);
            self.dispatcher
                .apply_resource_change(ResourceChange::EventDelete, self.resource_kind, old);
            cache = self.cache.lock().await;
        }
        cache.insert(key, value.clone());
        drop(cache);

        self.dispatcher
            .apply_resource_change(ResourceChange::EventAdd, self.resource_kind, value);
    }

    async fn handle_delete(&self, key: String) {
        let mut cache = self.cache.lock().await;
        if let Some(old) = cache.remove(&key) {
            // Determine if this is a base conf resource
            // Base conf resources (GatewayClass, EdgionGatewayConfig, Gateway) are loaded via load_base, not through watch
            let is_base_conf = if let Some(kind) = ResourceKind::from_content(&old) {
                matches!(
                    kind,
                    ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway
                )
            } else {
                false
            };

            drop(cache);
            
            // Skip base conf resources as they are handled by load_base
            if !is_base_conf {
                self.dispatcher
                    .apply_resource_change(ResourceChange::EventDelete, self.resource_kind, old);
            }
        }
    }
}

#[async_trait::async_trait]
impl ConfigLoader for EtcdConfigLoader {
    /// Register a dispatcher for handling configuration events
    /// Note: EtcdConfigLoader currently requires dispatcher at construction time
    async fn register_dispatcher(&self, _dispatcher: Arc<dyn ConfigServerEventDispatcher>) {
        tracing::warn!(
            component = "etcd_loader",
            "EtcdConfigLoader does not support late dispatcher registration. Dispatcher must be provided at construction."
        );
    }

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

    /// Bootstrap and load user configuration resources (all other resources)
    async fn bootstrap_user_conf(&self) -> Result<()> {
        let mut client_guard = self.client.lock().await;
        let client = client_guard.as_mut().ok_or_else(|| anyhow!("Client not connected"))?;

        let options = GetOptions::new().with_prefix();
        let resp = client
            .get(self.prefix.clone(), Some(options))
            .await
            .with_context(|| format!("Failed to fetch initial keys for prefix {}", self.prefix))?;

        let mut cache_guard = self.cache.lock().await;

        for kv in resp.kvs() {
            if let Ok(value) = String::from_utf8(kv.value().to_vec()) {
                // Only process non-base-conf resources
                let is_base_conf = if let Some(kind) = ResourceKind::from_content(&value) {
                    matches!(
                        kind,
                        ResourceKind::GatewayClass | ResourceKind::EdgionGatewayConfig | ResourceKind::Gateway
                    )
                } else {
                    false
                };

                if !is_base_conf {
                    let key = String::from_utf8_lossy(kv.key()).to_string();
                    cache_guard.insert(key, value.clone());
                    drop(cache_guard);
                    // Use InitAdd for bootstrap phase
                    self.dispatcher
                        .apply_resource_change(ResourceChange::InitAdd, None, value);
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
        let client = client_guard.as_mut().ok_or_else(|| anyhow!("Client not connected"))?;

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

    async fn set_enable_resource_version_fix(&self) {
        // not need for etcd loader
    }

    async fn load_base(&self, gateway_class_name: &str) -> Result<crate::core::conf_sync::GatewayBaseConf> {
        use crate::types::{GatewayClass, EdgionGatewayConfig, Gateway, ResourceKind};
        use crate::core::conf_sync::GatewayBaseConf;
        
        tracing::info!(
            "Starting to load base configuration from etcd for gateway_class_name: {}",
            gateway_class_name
        );
        
        // Step 1: Collect all base resources in one pass
        let mut gateway_classes: Vec<GatewayClass> = Vec::new();
        let mut edgion_gateway_configs: Vec<EdgionGatewayConfig> = Vec::new();
        let mut gateways: Vec<Gateway> = Vec::new();
        
        // Get all keys with prefix from etcd
        let mut client_guard = self.client.lock().await;
        let client = client_guard.as_mut().ok_or_else(|| anyhow!("Client not connected"))?;
        
        let options = GetOptions::new().with_prefix();
        let resp = client
            .get(self.prefix.clone(), Some(options))
            .await
            .context("Failed to get keys from etcd")?;
        
        drop(client_guard);
        
        // Traverse all keys and collect resources
        for kv in resp.kvs() {
            let key = String::from_utf8_lossy(kv.key()).to_string();
            let value = match String::from_utf8(kv.value().to_vec()) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("Failed to parse value for key {}: {}", key, e);
                    continue;
                }
            };
            
            // Determine resource kind
            let kind = match ResourceKind::from_content(&value) {
                Some(k) => k,
                None => {
                    tracing::warn!("Failed to determine resource kind for key {}", key);
                    continue;
                }
            };
            
            // Parse based on kind
            match kind {
                ResourceKind::GatewayClass => {
                    if let Ok(gc) = serde_yaml::from_str::<GatewayClass>(&value) {
                        gateway_classes.push(gc);
                    }
                }
                ResourceKind::EdgionGatewayConfig => {
                    if let Ok(egwc) = serde_yaml::from_str::<EdgionGatewayConfig>(&value) {
                        edgion_gateway_configs.push(egwc);
                    }
                }
                ResourceKind::Gateway => {
                    if let Ok(gw) = serde_yaml::from_str::<Gateway>(&value) {
                        gateways.push(gw);
                    }
                }
                _ => {
                    // Skip other resource types
                }
            }
        }
        
        tracing::debug!(
            "Collected resources from etcd: {} GatewayClasses, {} EdgionGatewayConfigs, {} Gateways",
            gateway_classes.len(),
            edgion_gateway_configs.len(),
            gateways.len()
        );
        
        // Step 2: Find the matching GatewayClass by name
        let gateway_class = gateway_classes
            .into_iter()
            .find(|gc| gc.metadata.name.as_ref().map(|n| n == gateway_class_name).unwrap_or(false))
            .ok_or_else(|| {
                anyhow!(
                    "GatewayClass '{}' not found in etcd",
                    gateway_class_name
                )
            })?;
        
        tracing::info!("Found matching GatewayClass: {:?}", gateway_class.metadata.name);
        
        // Step 3: Extract EdgionGatewayConfig name from GatewayClass parameters_ref
        let egwc_name = gateway_class
            .spec
            .parameters_ref
            .as_ref()
            .and_then(|params_ref| {
                if params_ref.kind == "EdgionGatewayConfig" {
                    Some(params_ref.name.clone())
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                anyhow!(
                    "GatewayClass '{}' does not reference an EdgionGatewayConfig via parameters_ref",
                    gateway_class_name
                )
            })?;
        
        tracing::info!(
            "GatewayClass references EdgionGatewayConfig: {}",
            egwc_name
        );
        
        // Step 4: Find the matching EdgionGatewayConfig by name
        let edgion_gateway_config = edgion_gateway_configs
            .into_iter()
            .find(|egwc| egwc.metadata.name.as_ref().map(|n| n == &egwc_name).unwrap_or(false))
            .ok_or_else(|| {
                anyhow!(
                    "EdgionGatewayConfig '{}' not found in etcd",
                    egwc_name
                )
            })?;
        
        tracing::info!("Found matching EdgionGatewayConfig: {:?}", edgion_gateway_config.metadata.name);
        
        // Step 5: Filter Gateways by gateway_class_name
        let matching_gateways: Vec<Gateway> = gateways
            .into_iter()
            .filter(|gw| gw.spec.gateway_class_name == gateway_class_name)
            .collect();
        
        tracing::info!(
            "Found {} matching Gateways for gateway_class_name: {}",
            matching_gateways.len(),
            gateway_class_name
        );
        
        tracing::info!(
            "Successfully loaded base configuration from etcd: GatewayClass={:?}, EdgionGatewayConfig={:?}, Gateways count={}",
            gateway_class.metadata.name,
            edgion_gateway_config.metadata.name,
            matching_gateways.len()
        );
        
        Ok(GatewayBaseConf::new(gateway_class, edgion_gateway_config, matching_gateways))
    }
}
