use crate::core::conf_load::LoaderArgs;
use crate::core::conf_sync::config_client::ConfigClient;
use crate::core::conf_sync::grpc_client::ConfigSyncClient;
use crate::core::utils::net::{normalize_grpc_endpoint, parse_optional_listen_addr};
use anyhow::{anyhow, Context, Result};
use clap::Parser;
use std::time::Duration;
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
        let _config_client = ConfigClient::new(self.gateway_class.clone());

        let server_addr = self.server_addr.as_deref()
            .ok_or_else(|| anyhow!("server_addr is required, please provide --server-addr"))?;

        let mut sync_client = ConfigSyncClient::new(
            server_addr,
            self.gateway_class.clone(),
            "edgion-gateway".to_string(),
            Duration::from_secs(10),
        );
        
        sync_client.connect().await?;

        sync_client.init().await?;

        Ok(())
    }
}
