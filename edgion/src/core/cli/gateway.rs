use super::admin::spawn_gateway_admin_server;
use super::{normalize_grpc_endpoint, parse_optional_listen_addr, GatewayCommand};
use crate::core::conf_sync::grpc_client::ConfigSyncClient;
use anyhow::{Context, Result};
use tokio::signal;

pub async fn run(cmd: &GatewayCommand) -> Result<()> {
    let server_endpoint = normalize_grpc_endpoint(&cmd.server_addr);
    let mut client = ConfigSyncClient::connect(server_endpoint.clone(), cmd.gateway_class.clone())
        .await
        .with_context(|| format!("failed to connect to operator at {}", server_endpoint))?;

    client
        .sync_all()
        .await
        .context("failed to perform initial configuration sync")?;
    client
        .start_watch_all()
        .await
        .context("failed to start configuration watches")?;

    let config_client = client.get_config_client();
    let admin_handle = match parse_optional_listen_addr(cmd.admin_listen.as_ref())? {
        Some(addr) => {
            println!("[gateway] admin HTTP address: {}", addr);
            Some(spawn_gateway_admin_server(config_client.clone(), addr))
        }
        None => None,
    };

    if let Some(addr) = &cmd.grpc_listen {
        println!("[gateway] gRPC listen address: {}", addr);
    }

    println!("[gateway] connected to operator {}", server_endpoint);
    println!("[gateway] press Ctrl+C to stop");

    signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl_c signal");

    if let Some(handle) = admin_handle {
        handle.shutdown().await;
    }

    Ok(())
}
