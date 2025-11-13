use super::admin::{spawn_gateway_admin_server, spawn_operator_admin_server};
use super::runtime::ConfigServerBridge;
use super::{
    default_embedded_operator_addr, normalize_grpc_endpoint, parse_listen_addr,
    parse_optional_listen_addr, resolve_filesystem_dir, GatewayWithOperatorCommand, LoaderArgs,
    LoaderKind,
};
use crate::core::conf_load::file_system::FileSystemConfigLoader;
use crate::core::conf_sync::config_server::ConfigServer;
use crate::core::conf_sync::grpc_client::ConfigSyncClient;
use crate::core::conf_sync::grpc_server::ConfigSyncServer;
use anyhow::{anyhow, Context, Result};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::signal;
use tokio::sync::oneshot;
use tonic::transport::Server;

pub async fn run(cmd: &GatewayWithOperatorCommand) -> Result<()> {
    let config_dir = resolve_filesystem_dir(&cmd.loader)?;
    ensure_filesystem_only(&cmd.loader)?;

    let operator_listen =
        parse_listen_addr(cmd.grpc_listen.as_ref(), default_embedded_operator_addr())?;
    let gateway_admin_addr = parse_optional_listen_addr(cmd.admin_listen.as_ref())?;
    let operator_admin_addr = parse_optional_listen_addr(cmd.operator_admin_listen.as_ref())?;

    let bridge = ConfigServerBridge::new();
    bridge.ensure_default_gateway_class().await;
    bridge.ensure_gateway_class(&cmd.gateway_class).await;

    let loader = FileSystemConfigLoader::new(config_dir.clone(), bridge.dispatcher(), None);
    let loader_handle = loader.spawn();

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let operator_server_handle =
        spawn_embedded_operator(bridge.server(), operator_listen, shutdown_rx);

    println!(
        "[gateway-with-operator] embedded operator gRPC address: {}",
        operator_listen
    );

    let operator_admin_handle = operator_admin_addr.map(|addr| {
        println!(
            "[gateway-with-operator] embedded operator admin HTTP address: {}",
            addr
        );
        spawn_operator_admin_server(bridge.server(), addr)
    });

    let operator_endpoint = normalize_grpc_endpoint(&operator_listen.to_string());

    let mut client =
        ConfigSyncClient::connect(operator_endpoint.clone(), cmd.gateway_class.clone())
            .await
            .context("failed to connect to embedded operator")?;

    client
        .sync_all()
        .await
        .context("failed to perform initial configuration sync")?;
    client
        .start_watch_all()
        .await
        .context("failed to start configuration watches")?;

    let config_client = client.get_config_client();
    let gateway_admin_handle = gateway_admin_addr.map(|addr| {
        println!(
            "[gateway-with-operator] gateway admin HTTP address: {}",
            addr
        );
        spawn_gateway_admin_server(config_client.clone(), addr)
    });

    println!(
        "[gateway-with-operator] configuration directory: {}",
        config_dir.display()
    );
    println!("[gateway-with-operator] press Ctrl+C to stop");

    signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl_c signal");

    let _ = shutdown_tx.send(());
    operator_server_handle.abort();
    let _ = operator_server_handle.await;

    if let Some(handle) = operator_admin_handle {
        handle.shutdown().await;
    }
    if let Some(handle) = gateway_admin_handle {
        handle.shutdown().await;
    }

    loader_handle.abort();
    let _ = loader_handle.await;

    Ok(())
}

fn ensure_filesystem_only(args: &LoaderArgs) -> Result<()> {
    if args.loader != LoaderKind::Filesystem {
        return Err(anyhow!(
            "gateway-with-operator currently only supports the filesystem loader"
        ));
    }
    Ok(())
}

fn spawn_embedded_operator(
    server: Arc<tokio::sync::Mutex<ConfigServer>>,
    addr: SocketAddr,
    shutdown: oneshot::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let shutdown_future = async {
            let _ = shutdown.await;
            println!("[gateway-with-operator] shutting down embedded operator");
        };

        let service = ConfigSyncServer::new_with_shared(server).into_service();

        if let Err(err) = Server::builder()
            .add_service(service)
            .serve_with_shutdown(addr, shutdown_future)
            .await
        {
            eprintln!("embedded operator gRPC server stopped with error: {}", err);
        }
    })
}
