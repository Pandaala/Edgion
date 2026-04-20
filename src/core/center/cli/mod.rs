use anyhow::Result;
use clap::Parser;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::watch;

use crate::core::center::aggregator::ResourceAggregator;
use crate::core::center::api::{router, ApiState};
use crate::core::center::commander::Commander;
use crate::core::center::config::CenterConfig;
use crate::core::center::fed_sync::registry::ControllerRegistry;
use crate::core::center::fed_sync::server::FederationGrpcServer;
use crate::core::center::metadata_store::CenterMetaDataStore;
use crate::core::center::proxy::{PendingProxyMap, ProxyForwarder};
use crate::core::center::scheduler;
use crate::core::center::watch_cache::{CenterSyncClient, CenterWatchCacheRegistry};
use crate::core::common::fed_sync::proto::federation_sync_server::FederationSyncServer;
use parking_lot::Mutex;

#[derive(Parser, Debug)]
#[command(name = "edgion-center", version, about = "Edgion Federated Center")]
pub struct EdgionCenterCli {
    #[arg(short = 'c', long, default_value = "config/edgion-center.yaml")]
    pub config_file: String,
}

impl EdgionCenterCli {
    pub fn parse_args() -> Self {
        Self::parse()
    }

    pub async fn run(&self) -> Result<()> {
        // Install the process-wide Prometheus recorder before anything emits
        // metrics. Idempotent, so tests and combined modes do not panic on
        // re-install.
        if let Err(e) = crate::core::common::observe::metrics_api::install_global_recorder(
            crate::core::common::observe::metrics_api::RecorderConfig {
                service: "edgion-center",
                idle_timeout_secs: 0,
                histogram_buckets: &[],
            },
        ) {
            tracing::error!(
                component = "center",
                event = "metrics_recorder_install_failed",
                error = %e,
                "Failed to install Prometheus recorder; /metrics will return 500"
            );
        }

        let content = match std::fs::read_to_string(&self.config_file) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    component = "center",
                    path = %self.config_file,
                    error = %e,
                    "Config file not readable, using defaults"
                );
                String::new()
            }
        };
        let config: CenterConfig = {
            let de = serde_yaml::Deserializer::from_str(&content);
            match serde_yaml::with::singleton_map_recursive::deserialize(de) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        component = "center",
                        path = %self.config_file,
                        error = %e,
                        "Config parse error, using defaults"
                    );
                    CenterConfig::default()
                }
            }
        };

        let registry = ControllerRegistry::new();
        let aggregator = Arc::new(ResourceAggregator::new());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let pending_proxies: PendingProxyMap = Arc::new(Mutex::new(HashMap::new()));

        let metadata_store = Arc::new(CenterMetaDataStore::new());
        let sync_client = Arc::new(CenterSyncClient {
            plugin_metadata: CenterWatchCacheRegistry::new(metadata_store.clone()),
        });

        let db: Option<Arc<crate::core::center::db::CenterDb>> = if config.database.enabled {
            match crate::core::center::db::CenterDb::open(&config.database.sqlite_path) {
                Ok(d) => {
                    tracing::info!(component = "center", path = %config.database.sqlite_path, "SQLite DB opened");
                    Some(Arc::new(d))
                }
                Err(e) => {
                    tracing::error!(component = "center", error = %e, "Failed to open SQLite DB, running without persistence");
                    None
                }
            }
        } else {
            None
        };

        let grpc_server = FederationGrpcServer::new(
            registry.clone(),
            aggregator.clone(),
            pending_proxies.clone(),
            config.sync.clone(),
            sync_client.clone(),
            db.clone(),
        );
        let pending_lists = grpc_server.pending_lists.clone();
        let pending_commands = grpc_server.pending_commands.clone();

        let commander = Arc::new(Commander::new(
            registry.clone(),
            pending_commands,
            config.sync.command_timeout_secs,
        ));

        let proxy = Arc::new(ProxyForwarder::new(
            registry.clone(),
            pending_proxies,
            config.sync.command_timeout_secs,
        ));

        let api_state = ApiState {
            aggregator: aggregator.clone(),
            commander,
            proxy: proxy.clone(),
            db: db.clone(),
            metadata_store: metadata_store.clone(),
            sync_client,
            registry: registry.clone(),
        };
        let http_addr: std::net::SocketAddr = config.server.http_addr.parse()?;
        let grpc_addr: std::net::SocketAddr = config.server.grpc_addr.parse()?;

        tracing::info!(component = "center", grpc_addr = %grpc_addr, http_addr = %http_addr, "Starting edgion-center");

        // Validate security config early
        config
            .grpc_security
            .validate()
            .map_err(|e| anyhow::anyhow!("Invalid grpc_security config: {}", e))?;

        // Build gRPC server with optional TLS
        let mut server_builder = tonic::transport::Server::builder();
        if let Some(tls_cfg) = config.grpc_security.resolve_active_tls() {
            if tls_cfg.skip_tls {
                tracing::error!(
                    component = "center",
                    "grpc_security skip_tls=true: starting WITHOUT TLS. INSECURE, emergency use only!"
                );
            } else {
                let server_tls = crate::core::common::grpc_tls::load_server_tls(tls_cfg).await?;
                server_builder = server_builder
                    .tls_config(server_tls)
                    .map_err(|e| anyhow::anyhow!("Failed to configure center gRPC TLS: {}", e))?;
                tracing::info!(component = "center", "Federation gRPC mTLS enabled");
            }
        }

        // gRPC server
        let grpc_handle = tokio::spawn(
            server_builder
                .add_service(FederationSyncServer::new(grpc_server))
                .serve(grpc_addr),
        );

        // HTTP Admin API — unified auth supports both OIDC and local auth
        let auth_config = config.auth.clone();
        // If the operator did not supply `[local_auth]`, fall back to the documented
        // dev-default (admin/edgion1234) with a per-boot random JWT secret so existing
        // tokens become invalid on restart. Previously the fallback was
        // `LocalAuthConfig::default()`, which has empty password+jwt_secret; after
        // fix #01's stricter validation that fallback would panic at startup.
        let local_auth_config = config.local_auth.clone().or_else(|| {
            tracing::info!(
                component = "center",
                "No local_auth configured, enabling default local auth (admin/edgion1234); JWT secret is random per boot"
            );
            Some(crate::core::common::local_auth::LocalAuthConfig {
                enabled: true,
                username: "admin".to_string(),
                password: "edgion1234".to_string(),
                jwt_secret: crate::core::common::local_auth::secret::generate_random_string(32),
                jwt_expiry_hours: 24,
                skip_paths: vec!["/health".to_string(), "/ready".to_string()],
            })
        });
        let http_handle = tokio::spawn(async move {
            // Build state first, wire routes, wrap with auth layer LAST. See
            // `common::api::wrap_with_unified_auth_layer` docs: `.layer()` only
            // protects routes present at the time of the call, so anything
            // merged after would silently bypass auth.
            let local_auth_intent = local_auth_config.as_ref().is_some_and(|c| c.enabled);
            let require_auth = auth_config.as_ref().is_some_and(|c| c.enabled) || local_auth_intent;
            let auth_state = crate::core::common::unified_auth::UnifiedAuthState::from_configs(
                auth_config.as_ref(),
                local_auth_config.as_ref(),
                require_auth,
                "center",
            )
            .expect("center auth state build failed");
            let base_router = router(api_state);
            let app =
                crate::core::common::api::add_local_auth_routes(base_router, auth_state.clone(), local_auth_intent);
            let app = crate::core::common::api::add_auth_status_route(app, auth_state.clone());
            let app = crate::core::common::api::wrap_with_unified_auth_layer(app, auth_state);
            let app = crate::core::common::api::apply_cors_layer(app);
            let listener = tokio::net::TcpListener::bind(http_addr).await.unwrap();
            axum::serve(listener, app).await.unwrap();
        });

        // Scheduler
        let sched_handle = tokio::spawn(scheduler::run(
            registry.clone(),
            pending_lists,
            config.sync.clone(),
            shutdown_rx.clone(),
        ));

        // Wait for any task to exit or Ctrl-C
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!(component = "center", "Ctrl-C received, shutting down");
                let _ = shutdown_tx.send(true);
            }
            r = grpc_handle => { tracing::error!(component = "center", "gRPC server exited: {:?}", r); }
            r = http_handle => { tracing::error!(component = "center", "HTTP server exited: {:?}", r); }
            r = sched_handle => { tracing::error!(component = "center", "Scheduler exited: {:?}", r); }
        }

        Ok(())
    }
}
