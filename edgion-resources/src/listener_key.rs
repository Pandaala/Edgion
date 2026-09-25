//! ListenerKey — process-local listener identity type.
//!
//! Format: `"{addr}/{PROTOCOL}"`, e.g. `"0.0.0.0:80/HTTP"`.
//! Used as primary key for all per-listener data structures.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::resources::gateway::Gateway;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Process-local listener identity: "{addr}/{protocol}", e.g. "0.0.0.0:80/HTTP".
/// The address is the Gateway's local bind IP when configured, otherwise a
/// canonical wildcard. It is independent of `Gateway.spec.addresses`.
/// Used as primary key for all per-listener data structures.
#[derive(Clone, Hash, Eq, PartialEq, Debug, Serialize, Deserialize, JsonSchema, Ord, PartialOrd)]
pub struct ListenerKey(pub String);

impl ListenerKey {
    /// Create a new `ListenerKey` from a socket address and protocol.
    ///
    /// The protocol is uppercased automatically, so `"http"` becomes `"HTTP"`.
    pub fn new(addr: SocketAddr, protocol: &str) -> Self {
        ListenerKey(format!("{}/{}", addr, protocol.to_uppercase()))
    }

    /// Returns the protocol portion of the key (everything after the last `/`).
    pub fn protocol(&self) -> &str {
        match self.0.rfind('/') {
            Some(pos) => &self.0[pos + 1..],
            None => "",
        }
    }
}

impl fmt::Display for ListenerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for ListenerKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Parse the optional Gateway-local bind IP. Callers that accept resources must
/// reject an invalid value before publishing listener or route state.
pub fn gateway_local_bind_ip(gateway: &Gateway) -> Result<Option<IpAddr>, std::net::AddrParseError> {
    gateway
        .metadata
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get(crate::constants::annotations::edgion::LOCAL_BIND_IP))
        .map(|value| {
            value.parse::<IpAddr>().map(|ip| match ip {
                IpAddr::V6(ipv6) => ipv6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(ipv6)),
                other => other,
            })
        })
        .transpose()
}

/// Resolve a listener's logical identity from its local bind IP. The process
/// default can replace the wildcard at socket creation without changing this
/// logical key; one process-wide default still merges shared listeners.
pub fn resolve_listener_addr(gateway: &Gateway, listener_port: i32) -> SocketAddr {
    SocketAddr::new(
        gateway_local_bind_ip(gateway)
            .ok()
            .flatten()
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        listener_port as u16,
    )
}

#[cfg(test)]
#[path = "../tests/unit/listener_key_test.rs"]
mod tests;
