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

        // Check if this is an IPv6 EndpointSlice
        let is_ipv6 = self.address_type == "IPv6";

        // Iterate through all endpoints in the slice
        for endpoint in &self.endpoints {
            // Check if endpoint is ready
            let is_ready = endpoint.conditions.as_ref().and_then(|c| c.ready).unwrap_or(false);

            if !is_ready {
                continue;
            }

            // Add all addresses from this endpoint
            for address in &endpoint.addresses {
                // Format address with port, IPv6 addresses need brackets
                let addr_with_port = if is_ipv6 {
                    // IPv6 address - wrap in brackets
                    format!("[{}]:{}", address, port)
                } else {
                    // IPv4 or FQDN address
                    format!("{}:{}", address, port)
                };

                if let Ok(socket_addr) = addr_with_port.parse::<SocketAddr>() {
                    let backend = Backend {
                        addr: socket_addr,
                        weight: 1,               // Default weight
                        ext: Default::default(), // Extension data
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
