use crate::core::conf_load::{Loader, LoaderArgs, LoaderKind};
use crate::core::model::edgion_op::EdgionOpServer;
use crate::core::utils::net::{
    default_operator_addr, parse_listen_addr, parse_optional_listen_addr,
};
use anyhow::{anyhow, Result};
use clap::Parser;

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
        let server = EdgionOpServer::new();

        let loader = Loader::from_args(&self.loader, server.config_server())?;

        // TODO: Run the loader
        // loader.run().await?;

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
