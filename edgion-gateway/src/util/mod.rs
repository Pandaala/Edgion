//! Gateway data-plane utility helpers.
//!
//! These modules were previously housed in `edgion-common::utils`, but their
//! only consumers are the gateway data plane (net address parsing, real-IP
//! extraction, PROXY-protocol v2 framing). They live here so the crate
//! boundary reflects actual ownership.

pub mod net;
pub use net::*;

pub mod real_ip_extractor;
pub use real_ip_extractor::{extract_ip_string, RealIpExtractor};

pub mod proxy_protocol;
