use crate::core::cli::admin::{spawn_gateway_admin_server, spawn_operator_admin_server};
use crate::core::cli::common::{
    default_embedded_operator_addr, normalize_grpc_endpoint, parse_listen_addr,
    parse_optional_listen_addr, resolve_filesystem_dir, LoaderArgs, LoaderKind,
};
use crate::core::cli::runtime::ConfigServerBridge;
use crate::core::conf_load::file_system::FileSystemConfigLoader;
use crate::core::conf_sync::config_server::ConfigServer;
use crate::core::conf_sync::grpc_client::ConfigSyncClient;
use crate::core::conf_sync::grpc_server::ConfigSyncServer;
use anyhow::{anyhow, Context, Result};
use clap::Parser;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::signal;
use tokio::sync::oneshot;
use tonic::transport::Server;

#[derive(Parser, Debug)]
#[command(
    name = "edgion-gateway",
    version,
    about = "Edgion Gateway standalone executable",
    long_about = None
)]
pub struct EdgionGwCli {
    /// Gateway class name
    #[arg(long, value_name = "CLASS")]
    pub gateway_class: String,

    /// Enable embedded operator mode
    #[arg(long)]
    pub with_operator: bool,

    /// External operator gRPC address (e.g., http://127.0.0.1:50061)
    /// Required when --with-operator is not set
    #[arg(long, value_name = "ADDR")]
    pub server_addr: Option<String>,

    /// Embedded operator gRPC listen address (only used with --with-operator)
    #[arg(long, value_name = "ADDR")]
    pub grpc_listen: Option<String>,

    /// Gateway admin HTTP listen address
    #[arg(long, value_name = "ADDR")]
    pub admin_listen: Option<String>,

    /// Embedded operator admin HTTP listen address (only used with --with-operator)
    #[arg(long, value_name = "ADDR")]
    pub operator_admin_listen: Option<String>,

    #[command(flatten)]
    pub loader: LoaderArgs,
}

impl EdgionGwCli {
    pub fn parse_args() -> Self {
        Self::parse()
    }

    pub async fn run(&self) -> Result<()> {
        if self.with_operator {
            self.run_with_operator().await
        } else {
            self.run_external().await
        }
    }

    async fn run_external(&self) -> Result<()> {
        let server_addr = self
            .server_addr
            .as_ref()
            .ok_or_else(|| anyhow!("--server-addr is required when --with-operator is not set"))?;

        let server_endpoint = normalize_grpc_endpoint(server_addr);
        let mut client =
            ConfigSyncClient::connect(server_endpoint.clone(), self.gateway_class.clone())
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
        let admin_handle = match parse_optional_listen_addr(self.admin_listen.as_ref())? {
            Some(addr) => {
                println!("[gateway] admin HTTP address: {}", addr);
                Some(spawn_gateway_admin_server(config_client.clone(), addr))
            }
            None => None,
        };

        if let Some(addr) = &self.grpc_listen {
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

    async fn run_with_operator(&self) -> Result<()> {
        let config_dir = resolve_filesystem_dir(&self.loader)?;
        ensure_filesystem_only(&self.loader)?;

        let operator_listen =
            parse_listen_addr(self.grpc_listen.as_ref(), default_embedded_operator_addr())?;
        let gateway_admin_addr = parse_optional_listen_addr(self.admin_listen.as_ref())?;
        let operator_admin_addr = parse_optional_listen_addr(self.operator_admin_listen.as_ref())?;

        let bridge = ConfigServerBridge::new();
        bridge.ensure_default_gateway_class().await;
        bridge.ensure_gateway_class(&self.gateway_class).await;

        let loader = FileSystemConfigLoader::new(config_dir.clone(), bridge.dispatcher(), None);
        let loader_handle = loader.spawn();

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let operator_server_handle =
            spawn_embedded_operator(bridge.server(), operator_listen, shutdown_rx);

        println!(
            "[gateway] embedded operator gRPC address: {}",
            operator_listen
        );

        let operator_admin_handle = operator_admin_addr.map(|addr| {
            println!("[gateway] embedded operator admin HTTP address: {}", addr);
            spawn_operator_admin_server(bridge.server(), addr)
        });

        let operator_endpoint = normalize_grpc_endpoint(&operator_listen.to_string());

        let mut client =
            ConfigSyncClient::connect(operator_endpoint.clone(), self.gateway_class.clone())
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
            println!("[gateway] gateway admin HTTP address: {}", addr);
            spawn_gateway_admin_server(config_client.clone(), addr)
        });

        println!(
            "[gateway] configuration directory: {}",
            config_dir.display()
        );
        println!("[gateway] press Ctrl+C to stop");

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
}

fn ensure_filesystem_only(args: &LoaderArgs) -> Result<()> {
    if args.loader != LoaderKind::Filesystem {
        return Err(anyhow!(
            "gateway with embedded operator currently only supports the filesystem loader"
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
            println!("[gateway] shutting down embedded operator");
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
