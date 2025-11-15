use crate::core::conf_load::{Loader, LoaderArgs, LoaderKind};
use crate::core::conf_sync::{ConfigServer, ConfigSyncServer};
use crate::core::utils;
use anyhow::{anyhow, Result};
use clap::Parser;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(
    name = "edgion-operator",
    version,
    about = "Edgion Operator standalone executable",
    long_about = None
)]
pub struct EdgionOpCli {
    /// Optional gRPC listen address for operator
    #[arg(long, value_name = "ADDR")]
    pub grpc_listen: Option<String>,

    /// Optional HTTP listen address for operator admin plane
    #[arg(long, value_name = "ADDR")]
    pub admin_listen: Option<String>,

    #[command(flatten)]
    pub loader: LoaderArgs,
}

impl EdgionOpCli {
    pub fn parse_args() -> Self {
        Self::parse()
    }

    pub async fn run(&self) -> Result<()> {
        let config_server = Arc::new(ConfigServer::new());
        let sync_server = ConfigSyncServer::new(config_server.clone());
        let loader = Loader::from_args(
            &self.loader,
            config_server as Arc<dyn crate::core::conf_sync::traits::EventDispatcher>,
        )?;

        let addr =
            utils::parse_listen_addr(self.grpc_listen.as_ref(), utils::DEFAULT_OPERATOR_GRPC_ADDR)?;

        // Run both services concurrently using tokio::join!
        let (sync_result, loader_result) = tokio::join!(
            sync_server.serve(addr),
            loader.run()
        );

        // Check results - if either service fails, return error
        sync_result.map_err(|e| anyhow!("gRPC server error: {}", e))?;
        loader_result?;

        Ok(())
    }
}

fn ensure_filesystem_only(args: &LoaderArgs) -> Result<()> {
    if args.loader != LoaderKind::Filesystem {
        return Err(anyhow!(
            "operator mode currently only supports the filesystem loader"
        ));
    }
    Ok(())
}
