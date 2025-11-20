use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::str::FromStr;

pub const DEFAULT_OPERATOR_GRPC_ADDR: &str = "127.0.0.1:50061";

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
