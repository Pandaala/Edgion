use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use kube::ResourceExt;
use pingora_core::server::Server;
use pingora_core::server::configuration::ServerConf;
use pingora_proxy::http_proxy_service;
use crate::core::gateway::edgion_http::EdgionHttp;
use crate::core::gateway::gateway_store::get_global_gateway_store;
use crate::core::routes::get_global_route_manager;
use crate::types::{GatewayBaseConf, ResourceMeta};
use anyhow::Result;
use crate::core::tls::tls_pingora::TlsCallback;

pub struct GatewayBase {
    base_conf: GatewayBaseConf,
    pingora_server: Mutex<Option<Server>>,
}

impl GatewayBase {
    pub fn new(base_conf: GatewayBaseConf) -> Self {
        Self {
            base_conf,
            pingora_server: Mutex::new(None),
        }
    }

    pub fn bootstrap(&self) -> Result<()> {
        tracing::info!("Bootstrapping gateways");
        
        // Read configuration from EdgionGatewayConfig, use defaults if not provided
        let server_conf = self.create_server_conf();
        let mut pingora_server = Server::new_with_opt_and_conf(None, server_conf);
        pingora_server.bootstrap();
        
        // Add all gateways to the global gateway store
        let gateway_store = get_global_gateway_store();
        {
            let mut store_guard = gateway_store.write().unwrap();
            for gateway in self.base_conf.gateways().iter() {
                if let Err(e) = store_guard.add_gateway(gateway.clone()) {
                    tracing::warn!("Failed to add gateway to store: {}", e);
                }
            }
        }

        for gateway in self.base_conf.gateways().iter() {
            // Prepare gateway metadata and routes before processing listeners
            let gateway_class_name = self.base_conf.gateway_class().metadata.name.clone();
            let gateway_namespace = gateway.metadata.namespace.clone();
            let gateway_name = gateway.name_any();
            
            // Get or create domain routes from global RouteManager
            // Use empty string for namespace if not present (key will be "/name")
            let route_manager = get_global_route_manager();
            let namespace_str = gateway.metadata.namespace.as_deref().unwrap_or("");
            let domain_routes = route_manager.get_or_create_domain_routes(namespace_str, &gateway_name);
            tracing::info!("Retrieved domain routes hook for gateway '{}'", gateway.key_name());

            // Process listeners
            if let Some(listeners) = &gateway.spec.listeners {
                for listener in listeners {
                    let listener = listener.clone();
                    let host = listener.hostname.as_deref().unwrap_or("0.0.0.0");
                    let addr = format!("{}:{}", host, listener.port);

                    let enable_tls = listener.tls.is_some() || listener.port == 443 || listener.port == 8443;

                    let edgion_http = EdgionHttp {
                        gateway_class_name: gateway_class_name.clone(),
                        gateway_namespace: gateway_namespace.clone(),
                        gateway_name: gateway_name.clone(),
                        listener,
                        server_start_time: SystemTime::now(),
                        server_header_opts: Default::default(),
                        ctx_cnt: Arc::new(Default::default()),
                        domain_routes: domain_routes.clone(),
                    };

                    let mut http_service = http_proxy_service(&pingora_server.configuration, edgion_http);

                    if enable_tls {
                        let tls_settings = TlsCallback::new_tls_settings_with_callback()?;
                        http_service.add_tls_with_settings(&addr, None, tls_settings);
                    } else {
                        http_service.add_tcp(&addr);
                    }

                    pingora_server.add_service(http_service);
                }
            }
        }
        
        // Save the configured server
        *self.pingora_server.lock().unwrap() = Some(pingora_server);
        
        Ok(())
    }

    fn create_server_conf(&self) -> ServerConf {
        let mut conf = ServerConf::default();
        let server_config = self.base_conf.edgion_gateway_config().spec.server.as_ref();
        
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

    pub fn run_forever(&self) {
        let mut server_guard = self.pingora_server.lock().unwrap();
        
        if let Some(pingora_server) = server_guard.take() {
            tracing::info!("Starting Pingora server...");
            pingora_server.run_forever();
        } else {
            panic!("Pingora server not initialized. Call bootstrap() first.");
        }
    }
}