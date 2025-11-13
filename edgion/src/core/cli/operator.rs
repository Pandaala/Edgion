use super::admin::spawn_operator_admin_server;
use super::runtime::ConfigServerBridge;
use super::{
    default_operator_addr, parse_listen_addr, parse_optional_listen_addr, resolve_filesystem_dir,
    LoaderArgs, LoaderKind, OperatorCommand,
};
use crate::core::conf_load::file_system::FileSystemConfigLoader;
use crate::core::conf_sync::config_server::ConfigServer;
use crate::core::conf_sync::grpc_server::ConfigSyncServer;
use anyhow::{anyhow, Result};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::signal;
use tonic::transport::Server;

pub async fn run(cmd: &OperatorCommand) -> Result<()> {
    let config_dir = resolve_filesystem_dir(&cmd.loader)?;
    let listen_addr = parse_listen_addr(cmd.grpc_listen.as_ref(), default_operator_addr())?;
    let admin_addr = parse_optional_listen_addr(cmd.admin_listen.as_ref())?;

    ensure_filesystem_only(&cmd.loader)?;

    let bridge = ConfigServerBridge::new();
    bridge.ensure_default_gateway_class().await;

    let loader = FileSystemConfigLoader::new(config_dir.clone(), bridge.dispatcher(), None);
    let loader_handle = loader.spawn();

    let admin_handle = admin_addr.map(|addr| spawn_operator_admin_server(bridge.server(), addr));

    println!(
        "[operator] configuration directory: {}",
        config_dir.display()
    );
    println!("[operator] gRPC listen address: {}", listen_addr);
    if let Some(addr) = admin_addr {
        println!("[operator] admin HTTP address: {}", addr);
    }

    serve_grpc(bridge.server(), listen_addr).await?;

    if let Some(handle) = admin_handle {
        handle.shutdown().await;
    }

    loader_handle.abort();
    let _ = loader_handle.await;

    Ok(())
}

fn ensure_filesystem_only(args: &LoaderArgs) -> Result<()> {
    if args.loader != LoaderKind::Filesystem {
        return Err(anyhow!(
            "operator mode currently only supports the filesystem loader"
        ));
    }
    Ok(())
}

async fn serve_grpc(server: Arc<tokio::sync::Mutex<ConfigServer>>, addr: SocketAddr) -> Result<()> {
    let shutdown_trigger = async {
        signal::ctrl_c()
            .await
            .expect("failed to listen for ctrl_c signal");
        println!("[operator] shutdown signal received");
    };

    let service = ConfigSyncServer::new_with_shared(server).into_service();

    Server::builder()
        .add_service(service)
        .serve_with_shutdown(addr, shutdown_trigger)
        .await
        .map_err(|err| err.into())
}
