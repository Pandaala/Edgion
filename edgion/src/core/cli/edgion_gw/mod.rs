use crate::core::conf_load::LoaderArgs;
use crate::core::conf_sync::grpc_client::ConfigSyncClient;
use crate::core::utils::net::{normalize_grpc_endpoint, parse_optional_listen_addr};
use anyhow::{anyhow, Context, Result};
use clap::Parser;
use tokio::signal;

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

    /// Operator gRPC address (e.g., http://127.0.0.1:50061)
    #[arg(long, value_name = "ADDR")]
    pub server_addr: Option<String>,

    /// Gateway admin HTTP listen address
    #[arg(long, value_name = "ADDR")]
    pub admin_listen: Option<String>,

    #[command(flatten)]
    pub loader: LoaderArgs,
}

impl EdgionGwCli {
    pub fn parse_args() -> Self {
        Self::parse()
    }

    pub async fn run(&self) -> Result<()> {
        self.run_external().await
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

        println!("[gateway] connected to operator {}", server_endpoint);
        println!("[gateway] press Ctrl+C to stop");

        signal::ctrl_c()
            .await
            .expect("failed to listen for ctrl_c signal");

        Ok(())
    }
}
