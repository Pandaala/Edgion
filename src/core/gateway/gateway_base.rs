use std::sync::{Arc, Mutex};
use kube::ResourceExt;
use pingora_core::server::Server;
use pingora_core::server::configuration::ServerConf;
use crate::core::conf_sync::conf_client::ConfigClient;
use crate::core::gateway::gateway_store::get_global_gateway_store;
use crate::core::gateway::listener_builder;
use crate::types::{ResourceMeta, Gateway, GatewayClass, EdgionGatewayConfig};
use anyhow::{Result, anyhow};
use crate::core::observe::AccessLogger;
use crate::core::link_sys::LocalFileWriter;
use crate::types::link_sys::LocalFileWriterConfig;

/// Create and initialize AccessLogger from configuration
/// Supports multiple output targets based on StringOutput enum
pub async fn create_access_logger(config: &crate::core::cli::edgion_gateway::config::AccessLogConfig) -> Result<Arc<AccessLogger>> {
    use crate::core::link_sys::DataSender;
    use crate::types::link_sys::StringOutput;
    
    let mut logger = AccessLogger::new();
    
    // Process output configuration based on variant
    match &config.output {
        StringOutput::LocalFile(file_cfg) => {
            // Check for environment variable override first
            let log_path = std::env::var("EDGION_ACCESS_LOG")
                .unwrap_or_else(|_| file_cfg.path.clone());
            
            // If path is empty, return empty logger
            if log_path.is_empty() {
                tracing::info!("Access logger disabled (no path configured)");
                return Ok(Arc::new(logger));
            }
            
            tracing::info!(
                path = %log_path,
                queue_size = ?file_cfg.queue_size,
                env_override = std::env::var("EDGION_ACCESS_LOG").is_ok(),
                "Initializing access logger with LocalFile output"
            );
            
            // Create LocalFileWriterConfig from config (with env override if present)
            let mut writer_config = LocalFileWriterConfig::new(&log_path);
            
            if let Some(queue_size) = file_cfg.queue_size {
                writer_config = writer_config.with_queue_size(queue_size);
            }
            
            if let Some(rotation) = &file_cfg.rotation {
                writer_config = writer_config.with_rotation(rotation.clone());
            }
            
            // Create and initialize LocalFileWriter
            let mut writer = LocalFileWriter::new(writer_config);
            writer.init().await?;
            
            // Register the writer
            logger.register(Box::new(writer));
            
            tracing::info!("Access logger initialized successfully with LocalFile output");
        }
        // Future: Add support for other output types
        // StringOutput::Es(es_cfg) => { ... }
        // StringOutput::Kafka(kafka_cfg) => { ... }
    }
    
    Ok(Arc::new(logger))
}

pub struct GatewayBase {
    config_client: Arc<ConfigClient>,
    access_logger: Arc<AccessLogger>,
    pingora_server: Mutex<Option<Server>>,
}

/// Parse HTTP/2 enable flag from Gateway annotations
/// Returns true if annotation is not set or set to "true"
/// Returns false if annotation is set to "false"
fn parse_enable_http2_annotation(gateway: &Gateway) -> bool {
    gateway.metadata.annotations.as_ref()
        .and_then(|annotations| annotations.get(listener_builder::ANNOTATION_ENABLE_HTTP2))
        .and_then(|value| {
            match value.to_lowercase().as_str() {
                "false" | "0" | "no" | "off" => Some(false),
                _ => Some(true),
            }
        })
        .unwrap_or(true) // Default to true if annotation is not present
}

impl GatewayBase {
    pub fn new(config_client: Arc<ConfigClient>, access_logger: Arc<AccessLogger>) -> Self {
        Self {
            config_client,
            access_logger,
            pingora_server: Mutex::new(None),
        }
    }
    
    /// Get the access logger
    fn get_access_logger(&self) -> Arc<AccessLogger> {
        self.access_logger.clone()
    }

    fn create_server_conf(&self, edgion_gateway_config: &EdgionGatewayConfig) -> ServerConf {
        let mut conf = ServerConf::default();
        let server_config = edgion_gateway_config.spec.server.as_ref();
        
        // Ensure daemon mode is disabled (we don't run as daemon)
        conf.daemon = false;
        
        // 1. Number of worker threads (default: number of CPU cores)
        conf.threads = server_config
            .and_then(|c| c.threads)
            .map(|t| t as usize)
            .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));
        
        // 2. Enable work stealing (default: true)
        conf.work_stealing = server_config
            .map(|c| c.work_stealing)
            .unwrap_or(true);
        
        // 3. Grace period for shutdown (default: 30 seconds)
        conf.grace_period_seconds = server_config
            .and_then(|c| c.grace_period_seconds)
            .or(Some(30));
        
        // 4. Graceful shutdown timeout (default: 10 seconds)
        conf.graceful_shutdown_timeout_seconds = server_config
            .and_then(|c| c.graceful_shutdown_timeout_s)
            .or(Some(10));
        
        // 5. Upstream keepalive pool size (default: 128)
        conf.upstream_keepalive_pool_size = server_config
            .and_then(|c| c.upstream_keepalive_pool_size)
            .map(|s| s as usize)
            .unwrap_or(128);
        
        // 6. Upstream connect timeout (default: 5 seconds) - Note: not available in ServerConf
        // This will be handled at the connection level
        
        // 7. Error log file path (default: None)
        conf.error_log = server_config
            .and_then(|c| c.error_log.clone());
        
        conf
    }

    /// Bootstrap: Create and configure Pingora server with all listeners
    /// Must be called from within a Tokio runtime context
    pub fn bootstrap(&self) -> Result<()> {
        tracing::info!("Bootstrapping gateways");
        
        // 1. Dynamically fetch all Gateway resources
        let gateways_data = self.config_client.list_gateways();
        let gateways = gateways_data.data;
        
        if gateways.is_empty() {
            return Err(anyhow!("No Gateway resources available for bootstrap"));
        }
        
        tracing::info!("Found {} Gateway resources", gateways.len());
        
        // 2. Get EdgionGatewayConfig from the first Gateway's GatewayClass for ServerConf
        let first_gateway = &gateways[0];
        let gateway_class_name = &first_gateway.spec.gateway_class_name;
        
        let gateway_class = self.config_client.get_gateway_class(gateway_class_name)
            .ok_or_else(|| anyhow!("GatewayClass '{}' not found", gateway_class_name))?;
        
        let edgion_gateway_config_name = gateway_class.spec.parameters_ref
            .as_ref()
            .ok_or_else(|| anyhow!("GatewayClass '{}' has no parametersRef", gateway_class_name))?
            .name.as_str();
        
        let edgion_gateway_config = self.config_client.get_edgion_gateway_config(edgion_gateway_config_name)
            .ok_or_else(|| anyhow!("EdgionGatewayConfig '{}' not found", edgion_gateway_config_name))?;
        
        // 3. Create and bootstrap Pingora Server
        let server_conf = self.create_server_conf(&edgion_gateway_config);
        let mut pingora_server = Server::new_with_opt_and_conf(None, server_conf);
        pingora_server.bootstrap();
        
        // 4. Add all gateways to the global gateway store
        let gateway_store = get_global_gateway_store();
        {
            let mut store_guard = gateway_store.write().unwrap();
            for gateway in gateways.iter() {
                if let Err(e) = store_guard.add_gateway(gateway.clone()) {
                    tracing::warn!("Failed to add gateway to store: {}", e);
                }
            }
        }
        
        // 5. Process each Gateway and configure listeners
        for gateway in gateways.iter() {
            // 5.1 Get GatewayClass for this Gateway
            let gateway_class_name = &gateway.spec.gateway_class_name;
            let gateway_class = match self.config_client.get_gateway_class(gateway_class_name) {
                Some(gc) => gc,
                None => {
                    tracing::warn!(
                        gateway = %gateway.key_name(),
                        gateway_class = %gateway_class_name,
                        "GatewayClass not found, skipping Gateway"
                    );
                    continue;
                }
            };
            
            // 5.2 Get EdgionGatewayConfig for this Gateway
            let edgion_gateway_config_name = match gateway_class.spec.parameters_ref.as_ref() {
                Some(params_ref) => &params_ref.name,
                None => {
                    tracing::warn!(
                        gateway = %gateway.key_name(),
                        gateway_class = %gateway_class_name,
                        "GatewayClass has no parametersRef, skipping Gateway"
                    );
                    continue;
                }
            };
            
            let edgion_gateway_config = match self.config_client.get_edgion_gateway_config(edgion_gateway_config_name) {
                Some(egwc) => egwc,
                None => {
                    tracing::warn!(
                        gateway = %gateway.key_name(),
                        edgion_gateway_config = %edgion_gateway_config_name,
                        "EdgionGatewayConfig not found, skipping Gateway"
                    );
                    continue;
                }
            };
            
            // 5.3 Process listeners for this Gateway
            if let Some(listeners) = &gateway.spec.listeners {
                tracing::info!(
                    "Processing gateway {} with {} listeners",
                    gateway.name_any(),
                    listeners.len()
                );
                
                // Prepare gateway metadata
                let gateway_class_name_clone = gateway_class.metadata.name.clone();
                let gateway_namespace = gateway.metadata.namespace.clone();
                let gateway_name = gateway.name_any();
                
                // Extract gateway annotations (convert BTreeMap to HashMap)
                let gateway_annotations = gateway.metadata.annotations.clone()
                    .map(|btree| btree.into_iter().collect::<std::collections::HashMap<_, _>>())
                    .unwrap_or_default();
                
                // Clone server configuration for listener building
                let server_conf = pingora_server.configuration.clone();
                
                // Parse HTTP/2 setting from Gateway annotation
                let enable_http2 = parse_enable_http2_annotation(&gateway);
                
                if !enable_http2 {
                    tracing::info!(
                        gateway = %gateway.key_name(),
                        "HTTP/2 disabled via annotation"
                    );
                }
                
                // Add each listener
                for listener in listeners {
                    tracing::info!(
                        "  Listener: name={}, protocol={}, port={}",
                        listener.name,
                        listener.protocol,
                        listener.port
                    );
                    
                    // Create listener context
                    let context = listener_builder::ListenerContext {
                        gateway_class_name: gateway_class_name_clone.clone(),
                        gateway_namespace: gateway_namespace.clone(),
                        gateway_name: gateway_name.clone(),
                        gateway_key: gateway.key_name(),
                        listener: listener.clone(),
                        access_logger: self.get_access_logger(),
                        edgion_gateway_config: Arc::new(edgion_gateway_config.clone()),
                        server_conf: server_conf.clone(),
                        enable_http2,
                        gateway_annotations: gateway_annotations.clone(),
                    };
                    
                    // Dispatch to appropriate listener builder based on protocol
                    listener_builder::add_listener(&mut pingora_server, context)?;
                }
            }
        }
        
        // 6. Save the configured server
        *self.pingora_server.lock().unwrap() = Some(pingora_server);
        
        tracing::info!("Gateway bootstrap completed successfully");
        
        Ok(())
    }

    /// Run the Pingora server forever (blocking call)
    /// Must be called after bootstrap() and from the main thread
    pub fn run_forever(&self) {
        let mut server_guard = self.pingora_server.lock().unwrap();
        
        if let Some(pingora_server) = server_guard.take() {
            // Print all listening addresses before starting
            tracing::info!("Starting Pingora server with the following listeners:");
            let gateways = self.config_client.list_gateways().data;
            for gateway in gateways.iter() {
                if let Some(listeners) = &gateway.spec.listeners {
                    for listener in listeners {
                        let host = listener.hostname.as_deref().unwrap_or("0.0.0.0");
                        let addr = format!("{}:{}", host, listener.port);
                        let protocol = &listener.protocol;
                        tracing::info!(
                            gateway=%gateway.key_name(),
                            listener=%listener.name,
                            addr=%addr,
                            protocol=%protocol,
                            "Listening on"
                        );
                    }
                }
            }
            pingora_server.run_forever();
        } else {
            panic!("Pingora server not initialized. Call bootstrap() first.");
        }
    }
}