use crate::core::conf_sync::config_client::ConfigClient;
use crate::core::conf_sync::grpc_client::ConfigSyncClient;
use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;

pub struct EdgionGw {
    sync_client: ConfigSyncClient,
}

impl EdgionGw {
    pub fn new(sync_client: ConfigSyncClient) -> Self {
        Self { sync_client }
    }

    pub async fn serve(&self) -> Result<()> {
        println!("[gateway] gateway started, waiting for shutdown signal");

        signal::ctrl_c()
            .await
            .expect("failed to listen for ctrl_c signal");

        println!("[gateway] shutdown signal received");
        Ok(())
    }

    pub async fn shutdown(&mut self) {
        // ConfigSyncClient will be automatically dropped when EdgionGw is dropped
    }

    pub fn config_client(&self) -> Arc<ConfigClient> {
        self.sync_client.get_config_client()
    }
}

pub async fn start(operator_addr: String, gateway_class: String) -> Result<()> {
    let mut client = ConfigSyncClient::new(
        operator_addr.as_str(),
        gateway_class.clone(),
        "edgion-gateway".to_string(),
        Duration::from_secs(10),
    );
    client.connect().await?;

    // Initialize base configuration and sync all resources
    client.init().await?;
    
    // Start watching for changes
    client.start_watch_all().await?;

    let mut gateway = EdgionGw::new(client);

    println!("[gateway] connected to operator at {}", operator_addr);

    gateway.serve().await?;
    gateway.shutdown().await;

    Ok(())
}
