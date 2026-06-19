//! Service discovery implementation for Kubernetes EndpointSlice
//!
//! This module provides the `EndpointSliceExt` trait for building Pingora backends
//! from Kubernetes EndpointSlice resources.

use k8s_openapi::api::discovery::v1::EndpointSlice;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_load_balancing::Backend;
use std::collections::BTreeSet;

/// Extension trait to add port information for service discovery
pub trait EndpointSliceExt {
    /// Build backends from EndpointSlice with specified port
    fn build_backends(&self, port: u16) -> BTreeSet<Backend>;
}

impl EndpointSliceExt for EndpointSlice {
    fn build_backends(&self, port: u16) -> BTreeSet<Backend> {
        let mut backends = BTreeSet::new();

        let is_ipv6 = self.address_type == "IPv6";

        for endpoint in &self.endpoints {
            let is_ready = endpoint.conditions.as_ref().and_then(|c| c.ready).unwrap_or(false);

            if !is_ready {
                continue;
            }

            for address in &endpoint.addresses {
                // IPv6 addresses require brackets: `[addr]:port`
                let addr_with_port = if is_ipv6 {
                    format!("[{}]:{}", address, port)
                } else {
                    format!("{}:{}", address, port)
                };

                if let Ok(socket_addr) = addr_with_port.parse::<SocketAddr>() {
                    let backend = Backend {
                        addr: socket_addr,
                        weight: 1,
                        ext: Default::default(),
                    };
                    backends.insert(backend);

                    tracing::debug!(
                        endpoint = %address,
                        port = port,
                        address_type = %self.address_type,
                        "Built backend from EndpointSlice"
                    );
                }
            }
        }

        if backends.is_empty() {
            tracing::warn!(
                endpoint_slice = ?self.metadata.name,
                "No ready backends found in EndpointSlice"
            );
        } else {
            tracing::debug!(
                endpoint_slice = ?self.metadata.name,
                backend_count = backends.len(),
                "Built backends from EndpointSlice"
            );
        }

        backends
    }
}
