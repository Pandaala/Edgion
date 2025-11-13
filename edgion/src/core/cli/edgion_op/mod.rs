use crate::core::model::edgion_op::EdgionOpServer;
use crate::core::model::edgion_op::{resolve_filesystem_dir, LoaderArgs, LoaderKind};
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
        let config_dir = resolve_filesystem_dir(&self.loader)?;
        let listen_addr = parse_listen_addr(self.grpc_listen.as_ref(), default_operator_addr())?;
        let admin_addr = parse_optional_listen_addr(self.admin_listen.as_ref())?;

        ensure_filesystem_only(&self.loader)?;

        let mut server = EdgionOpServer::new();

        server
            .run_with_admin(config_dir, listen_addr, admin_addr, |server, addr| {
                crate::core::model::edgion_op::admin::spawn_operator_admin_server(server, addr);
            })
            .await?;

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
