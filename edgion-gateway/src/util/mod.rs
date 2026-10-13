//! Gateway data-plane utility helpers: net address parsing, real-IP extraction,
//! and PROXY-protocol v2 framing.

pub mod net;
pub use net::*;

pub mod real_ip_extractor;
pub use real_ip_extractor::RealIpExtractor;

pub mod proxy_protocol;

pub mod client_ip;
pub use client_ip::{resolve_transport_client_addrs, TransportClientAddrs};

pub mod redacted;
pub use redacted::RedactedString;

pub(crate) mod http_body;
