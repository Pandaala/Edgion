use anyhow::{anyhow, Context, Result};
use clap::{Args, ValueEnum};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;

pub const DEFAULT_FILESYSTEM_DIR: &str = "edgion/config/examples";
pub const DEFAULT_OPERATOR_GRPC_ADDR: &str = "127.0.0.1:50061";
pub const DEFAULT_GATEWAY_EMBED_GRPC_ADDR: &str = "127.0.0.1:50062";

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
pub enum LoaderKind {
    Filesystem,
    Etcd,
}

#[derive(Args, Debug, Clone)]
pub struct LoaderArgs {
    /// Configuration loader type (currently only filesystem is supported)
    #[arg(long, value_enum, value_name = "TYPE", default_value = "filesystem")]
    pub loader: LoaderKind,

    /// Root directory for filesystem loader
    #[arg(long, value_name = "DIR")]
    pub dir: Option<String>,

    /// Etcd node addresses (not currently supported)
    #[arg(long = "etcd-endpoint", value_name = "URL")]
    pub etcd_endpoint: Vec<String>,

    /// Etcd key prefix (not currently supported)
    #[arg(long = "etcd-prefix", value_name = "PREFIX")]
    pub etcd_prefix: Option<String>,
}

pub fn resolve_filesystem_dir(args: &LoaderArgs) -> Result<PathBuf> {
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

pub fn parse_listen_addr(addr: Option<&String>, default: &str) -> Result<SocketAddr> {
    let candidate = addr.map(String::as_str).unwrap_or(default);
    SocketAddr::from_str(candidate)
        .with_context(|| format!("failed to parse listen address '{}'", candidate))
}

pub fn parse_optional_listen_addr(addr: Option<&String>) -> Result<Option<SocketAddr>> {
    addr.map(|value| {
        SocketAddr::from_str(value)
            .with_context(|| format!("failed to parse listen address '{}'", value))
    })
    .transpose()
}

pub fn normalize_grpc_endpoint(addr: &str) -> String {
    if addr.starts_with("http://") || addr.starts_with("https://") {
        addr.to_string()
    } else {
        format!("http://{}", addr)
    }
}

pub fn default_operator_addr() -> &'static str {
    DEFAULT_OPERATOR_GRPC_ADDR
}

pub fn default_embedded_operator_addr() -> &'static str {
    DEFAULT_GATEWAY_EMBED_GRPC_ADDR
}

