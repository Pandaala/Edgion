use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use tokio::sync::watch;

use crate::core::center::aggregator::ResourceAggregator;
use crate::core::center::api::{router, ApiState};
use crate::core::center::commander::Commander;
use crate::core::center::config::CenterConfig;
use crate::core::center::fed_sync::registry::ControllerRegistry;
use crate::core::center::fed_sync::server::FederationGrpcServer;
use crate::core::center::scheduler;
use crate::core::common::fed_sync::proto::federation_sync_server::FederationSyncServer;

#[derive(Parser, Debug)]
#[command(name = "edgion-center", version, about = "Edgion Federated Center")]
pub struct EdgionCenterCli {
    #[arg(short = 'c', long, default_value = "config/edgion-center.toml")]
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
        let config: CenterConfig = match toml::from_str(&content) {
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
        };

        let registry = ControllerRegistry::new();
        let aggregator = Arc::new(ResourceAggregator::new());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let grpc_server = FederationGrpcServer::new(registry.clone(), aggregator.clone(), config.sync.clone());
        let pending_lists = grpc_server.pending_lists.clone();
        let pending_commands = grpc_server.pending_commands.clone();

        let commander = Arc::new(Commander::new(
            registry.clone(),
            pending_commands,
            config.sync.command_timeout_secs,
        ));

        let api_state = ApiState {
            aggregator: aggregator.clone(),
            commander,
        };
        let http_addr: std::net::SocketAddr = config.server.http_addr.parse()?;
        let grpc_addr: std::net::SocketAddr = config.server.grpc_addr.parse()?;

        tracing::info!(component = "center", grpc_addr = %grpc_addr, http_addr = %http_addr, "Starting edgion-center");

        // gRPC server
        let grpc_handle = tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(FederationSyncServer::new(grpc_server))
                .serve(grpc_addr),
        );

        // HTTP Admin API
        let http_handle = tokio::spawn(async move {
            let listener = tokio::net::TcpListener::bind(http_addr).await.unwrap();
            axum::serve(listener, router(api_state)).await.unwrap();
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
