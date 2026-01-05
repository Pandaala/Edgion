//! Service discovery implementation for Kubernetes Endpoints
//!
//! This module implements Pingora's ServiceDiscovery trait directly for Endpoints,
//! allowing it to be used with Pingora's load balancing infrastructure.

use async_trait::async_trait;
use futures::FutureExt;
use k8s_openapi::api::core::v1::Endpoints;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_load_balancing::discovery::ServiceDiscovery;
use pingora_load_balancing::selection::BackendSelection;
use pingora_load_balancing::Backend;
use pingora_load_balancing::{Backends, LoadBalancer};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, RwLock};

/// Extension trait to add port information for service discovery
pub trait EndpointExt {
    /// Build backends from Endpoints with specified port
    fn build_backends(&self, port: u16) -> BTreeSet<Backend>;
}

impl EndpointExt for Endpoints {
    fn build_backends(&self, port: u16) -> BTreeSet<Backend> {
        let mut backends = BTreeSet::new();

        // Iterate through all subsets in Endpoints
        if let Some(subsets) = &self.subsets {
            for subset in subsets {
                // Get addresses from subset
                let addresses = if let Some(addrs) = &subset.addresses {
                    addrs
                } else {
                    continue;
                };

                // Check if we should use a specific port from subset.ports
                // If port is provided, use it; otherwise try to find matching port in subset
                let target_port = if port != 80 {
                    // Use provided port
                    port
                } else if let Some(ports) = &subset.ports {
                    // Use first port from subset
                    ports.first().and_then(|p| Some(p.port as u16)).unwrap_or(port)
                } else {
                    port
                };

                // Add all ready addresses from this subset
                for address in addresses {
                    let ip = &address.ip;

                    // Determine if this is IPv6
                    let is_ipv6 = ip.contains(':');

                    // Format address with port, IPv6 addresses need brackets
                    let addr_with_port = if is_ipv6 {
                        // IPv6 address - wrap in brackets
                        format!("[{}]:{}", ip, target_port)
                    } else {
                        // IPv4 address
                        format!("{}:{}", ip, target_port)
                    };

                    if let Ok(socket_addr) = addr_with_port.parse::<SocketAddr>() {
                        let backend = Backend {
                            addr: socket_addr,
                            weight: 1,               // Default weight
                            ext: Default::default(), // Extension data
                        };
                        backends.insert(backend);

                        tracing::debug!(
                            endpoint = %ip,
                            port = target_port,
                            "Built backend from Endpoints"
                        );
                    }
                }
            }
        }

        if backends.is_empty() {
            tracing::warn!(
                endpoint = ?self.metadata.name,
                "No ready backends found in Endpoints"
            );
        } else {
            tracing::debug!(
                endpoint = ?self.metadata.name,
                backend_count = backends.len(),
                "Built backends from Endpoints"
            );
        }

        backends
    }
}

/// Wrapper for Endpoints that implements ServiceDiscovery
///
/// This allows using Endpoints directly with Pingora's load balancing.
/// Provides interior mutability for Endpoints updates without cloning.
/// Note: This struct is typically wrapped in Arc at the storage layer.
#[derive(Clone)]
pub struct EndpointDiscovery {
    /// The Endpoints to discover backends from (with interior mutability)
    endpoint: Arc<RwLock<Endpoints>>,
}

impl EndpointDiscovery {
    /// Create a new EndpointDiscovery from Endpoints
    /// Returns Arc<Self> since it's typically used with Arc at storage layer
    pub fn new(endpoint: Endpoints) -> Self {
        Self {
            endpoint: Arc::new(RwLock::new(endpoint)),
        }
    }

    /// Get the port from Endpoints (returns first port or 80 as default)
    fn get_port(&self) -> u16 {
        let ep = self.endpoint.read().unwrap();
        ep.subsets
            .as_ref()
            .and_then(|subsets| subsets.first())
            .and_then(|subset| subset.ports.as_ref())
            .and_then(|ports| ports.first())
            .map(|p| p.port as u16)
            .unwrap_or(80)
    }

    /// Update the Endpoints data in-place
    /// This updates the Endpoints without replacing the entire EndpointDiscovery
    pub fn update(&self, new_endpoint: Endpoints) -> Result<(), String> {
        // Update endpoint
        *self.endpoint.write().unwrap() = new_endpoint;

        tracing::debug!("Updated EndpointDiscovery in-place");
        Ok(())
    }

    /// Execute a function with read access to the underlying Endpoints
    pub fn with_endpoint<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Endpoints) -> R,
    {
        let ep = self.endpoint.read().unwrap();
        f(&ep)
    }

    /// Get a clone of the underlying Endpoints
    pub fn endpoint(&self) -> Endpoints {
        self.endpoint.read().unwrap().clone()
    }
}

#[async_trait]
impl ServiceDiscovery for EndpointDiscovery {
    /// Discover backends from Endpoints
    ///
    /// This method is called by Pingora's load balancer to get the current
    /// list of available backends based on the Endpoints data.
    async fn discover(&self) -> Result<(BTreeSet<Backend>, HashMap<u64, bool>), Box<pingora_core::Error>> {
        let ep = self.endpoint.read().unwrap();
        let port = self.get_port();
        let backends = ep.build_backends(port);

        // Return empty health map - all backends default to healthy
        let health = HashMap::new();

        Ok((backends, health))
    }
}

/// EndpointLoadBalancer combines EndpointDiscovery with LoadBalancer
///
/// This struct wraps an EndpointDiscovery and creates a LoadBalancer that uses it
/// for service discovery. This is a generic struct that works with any BackendSelection algorithm.
pub struct EndpointLoadBalancer<S>
where
    S: BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    /// The discovery implementation
    discovery: EndpointDiscovery,
    /// The load balancer using the discovery
    lb: LoadBalancer<S>,
}

impl<S> EndpointLoadBalancer<S>
where
    S: BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    /// Create a new EndpointLoadBalancer from Endpoints
    /// Returns Arc<Self> since it's typically used with Arc at storage layer
    pub fn new(endpoint: Endpoints) -> Arc<Self> {
        let discovery = EndpointDiscovery::new(endpoint);

        let backends = Backends::new(Box::new(discovery.clone()));
        let lb = LoadBalancer::from_backends(backends);

        // Initialize backends by calling update once
        // This triggers the first discovery to populate backends
        match lb.update().now_or_never() {
            Some(Ok(_)) => {
                tracing::debug!("LoadBalancer initialized with backends");
            }
            Some(Err(e)) => {
                // Check if it's a "no backends" error (expected) or real error
                let err_msg = format!("{:?}", e);
                if err_msg.contains("empty") || err_msg.contains("no backend") {
                    tracing::debug!("LoadBalancer initialized with no backends (expected for empty Endpoints)");
                } else {
                    tracing::error!(
                        error = ?e,
                        "Unexpected error initializing LoadBalancer, this may cause issues"
                    );
                }
            }
            None => {
                // This should never happen for our discovery implementation
                tracing::error!("LoadBalancer update blocked - this indicates a bug in EndpointDiscovery");
            }
        }

        Arc::new(Self { discovery, lb })
    }

    /// Update the Endpoints data in-place
    pub fn update(&self, new_endpoint: Endpoints) -> Result<(), String> {
        self.discovery.update(new_endpoint)
    }

    /// Trigger LoadBalancer update
    /// Calls lb.update() which will refresh backends from discovery
    pub async fn update_load_balancer(&self) -> Result<(), String> {
        self.lb
            .update()
            .await
            .map_err(|e| format!("Failed to update LoadBalancer: {}", e))?;

        tracing::debug!("LoadBalancer updated");
        Ok(())
    }

    /// Get a reference to the load balancer
    pub fn load_balancer(&self) -> &LoadBalancer<S> {
        &self.lb
    }

    /// Execute a function with read access to the underlying Endpoints
    pub fn with_endpoint<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Endpoints) -> R,
    {
        self.discovery.with_endpoint(f)
    }

    /// Get a clone of the underlying Endpoints
    pub fn endpoint(&self) -> Endpoints {
        self.discovery.endpoint()
    }
}
