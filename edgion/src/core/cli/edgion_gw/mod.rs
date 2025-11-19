use crate::core::conf_load::LoaderArgs;
use crate::core::conf_sync::config_client::ConfigClient;
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
        let config_client = ConfigClient::new(self.gateway_class.clone());

        let sync_client = ConfigSyncClient::connect(self.server_addr.clone(), self.gateway_class.clone()).await?;

        Ok(())
    }
}
