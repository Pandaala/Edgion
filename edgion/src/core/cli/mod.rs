use anyhow::{anyhow, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;

mod admin;
mod gateway;
mod gateway_with_operator;
mod operator;
mod runtime;

const DEFAULT_FILESYSTEM_DIR: &str = "edgion/config/examples";
const DEFAULT_OPERATOR_GRPC_ADDR: &str = "127.0.0.1:50061";
const DEFAULT_GATEWAY_EMBED_GRPC_ADDR: &str = "127.0.0.1:50062";

#[derive(Parser, Debug)]
#[command(name = "edgion")]
#[command(version, about = "Edgion - High-performance API Gateway", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the operator (configuration server)
    Operator(OperatorCommand),

    /// Run the gateway in client mode connecting to an external operator
    Gateway(GatewayCommand),

    /// Run the gateway with an embedded operator and configuration loader
    GatewayWithOperator(GatewayWithOperatorCommand),
}

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
pub enum LoaderKind {
    Filesystem,
    Etcd,
}

#[derive(Args, Debug)]
pub struct OperatorCommand {
    /// Optional gRPC listen address for the operator
    #[arg(long, value_name = "ADDR")]
    pub grpc_listen: Option<String>,

    /// Optional admin HTTP listen address for operator inspection
    #[arg(long, value_name = "ADDR")]
    pub admin_listen: Option<String>,

    #[command(flatten)]
    pub loader: LoaderArgs,
}

#[derive(Args, Debug)]
pub struct GatewayCommand {
    /// Gateway class name
    #[arg(long, value_name = "CLASS")]
    pub gateway_class: String,

    /// Optional gRPC listen address for the gateway control plane
    #[arg(long, value_name = "ADDR")]
    pub grpc_listen: Option<String>,

    /// Connect to an external operator over gRPC (e.g. http://127.0.0.1:50061)
    #[arg(long, value_name = "ADDR")]
    pub server_addr: String,

    /// Optional admin HTTP listen address for gateway inspection
    #[arg(long, value_name = "ADDR")]
    pub admin_listen: Option<String>,
}

#[derive(Args, Debug)]
pub struct GatewayWithOperatorCommand {
    /// Gateway class name
    #[arg(long, value_name = "CLASS")]
    pub gateway_class: String,

    /// Optional gRPC listen address for the embedded operator
    #[arg(long, value_name = "ADDR")]
    pub grpc_listen: Option<String>,

    /// Optional admin HTTP listen address for gateway inspection
    #[arg(long, value_name = "ADDR")]
    pub admin_listen: Option<String>,

    /// Optional admin HTTP listen address for the embedded operator
    #[arg(long, value_name = "ADDR")]
    pub operator_admin_listen: Option<String>,

    #[command(flatten)]
    pub loader: LoaderArgs,
}

#[derive(Args, Debug)]
pub struct LoaderArgs {
    /// Configuration loader type (currently only filesystem is supported)
    #[arg(long, value_enum, value_name = "TYPE", default_value = "filesystem")]
    pub loader: LoaderKind,

    /// Root directory when using the filesystem loader
    #[arg(long, value_name = "DIR")]
    pub dir: Option<String>,

    /// Etcd endpoints (currently unsupported)
    #[arg(long = "etcd-endpoint", value_name = "URL")]
    pub etcd_endpoint: Vec<String>,

    /// Etcd key prefix (currently unsupported)
    #[arg(long = "etcd-prefix", value_name = "PREFIX")]
    pub etcd_prefix: Option<String>,
}

impl Cli {
    pub fn parse_args() -> Self {
        Self::parse()
    }

    pub async fn run(&self) -> Result<()> {
        match &self.command {
            Command::Operator(cmd) => operator::run(cmd).await,
            Command::Gateway(cmd) => gateway::run(cmd).await,
            Command::GatewayWithOperator(cmd) => gateway_with_operator::run(cmd).await,
        }
    }
}

pub(crate) fn resolve_filesystem_dir(args: &LoaderArgs) -> Result<PathBuf> {
    if args.loader != LoaderKind::Filesystem {
        return Err(anyhow!(
            "currently only the filesystem loader is supported by the CLI"
        ));
    }

    if !args.etcd_endpoint.is_empty() || args.etcd_prefix.is_some() {
        println!("[CLI] etcd loader options are ignored for the filesystem loader");
    }

    let dir = args
        .dir
        .clone()
        .unwrap_or_else(|| DEFAULT_FILESYSTEM_DIR.to_string());
    let path = PathBuf::from(&dir);
    if !path.exists() {
        return Err(anyhow!("configuration directory {:?} does not exist", path));
    }
    Ok(path)
}

pub(crate) fn parse_listen_addr(addr: Option<&String>, default: &str) -> Result<SocketAddr> {
    let candidate = addr.map(String::as_str).unwrap_or(default);
    SocketAddr::from_str(candidate)
        .with_context(|| format!("failed to parse listen address '{}'", candidate))
}

pub(crate) fn parse_optional_listen_addr(addr: Option<&String>) -> Result<Option<SocketAddr>> {
    addr.map(|value| {
        SocketAddr::from_str(value)
            .with_context(|| format!("failed to parse listen address '{}'", value))
    })
    .transpose()
}

pub(crate) fn normalize_grpc_endpoint(addr: &str) -> String {
    if addr.starts_with("http://") || addr.starts_with("https://") {
        addr.to_string()
    } else {
        format!("http://{}", addr)
    }
}

pub(crate) fn default_operator_addr() -> &'static str {
    DEFAULT_OPERATOR_GRPC_ADDR
}

pub(crate) fn default_embedded_operator_addr() -> &'static str {
    DEFAULT_GATEWAY_EMBED_GRPC_ADDR
}
