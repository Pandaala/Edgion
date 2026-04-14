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
use crate::core::center::proxy::{PendingProxyMap, ProxyForwarder};
use crate::core::center::scheduler;
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

        let grpc_server = FederationGrpcServer::new(
            registry.clone(),
            aggregator.clone(),
            pending_proxies.clone(),
            config.sync.clone(),
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

        let api_state = ApiState {
            aggregator: aggregator.clone(),
            commander,
            proxy: proxy.clone(),
            db: db.clone(),
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
        let local_auth_config = config.local_auth.clone().or_else(|| {
            tracing::info!(
                component = "center",
                "No local_auth configured, enabling default local auth (admin/edgion1234)"
            );
            Some(crate::core::common::local_auth::LocalAuthConfig::default())
        });
        let http_handle = tokio::spawn(async move {
            let base_router = router(api_state);
            let app = crate::core::common::api::add_local_auth_routes(base_router, local_auth_config.as_ref());
            let app = crate::core::common::api::apply_unified_auth_layer(
                app,
                auth_config.as_ref(),
                local_auth_config.as_ref(),
                "center",
            );
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

        let db_sync_handle = if let Some(db) = &db {
            let db = db.clone();
            let agg = aggregator.clone();
            let prx = proxy.clone();
            let interval = config.database.sync_interval_secs;
            let shutdown_rx2 = shutdown_rx.clone();
            Some(tokio::spawn(crate::core::center::db::sync::run_sync_task(
                agg,
                prx,
                db,
                interval,
                shutdown_rx2,
            )))
        } else {
            None
        };

        // Wait for any task to exit or Ctrl-C
        // We use a helper future so the optional db_sync_handle can be selected on.
        let db_sync_fut = async move {
            match db_sync_handle {
                Some(h) => h.await,
                None => std::future::pending().await,
            }
        };

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!(component = "center", "Ctrl-C received, shutting down");
                let _ = shutdown_tx.send(true);
            }
            r = grpc_handle => { tracing::error!(component = "center", "gRPC server exited: {:?}", r); }
            r = http_handle => { tracing::error!(component = "center", "HTTP server exited: {:?}", r); }
            r = sched_handle => { tracing::error!(component = "center", "Scheduler exited: {:?}", r); }
            r = db_sync_fut => { tracing::error!(component = "center", "DB sync task exited: {:?}", r); }
        }

        Ok(())
    }
}
