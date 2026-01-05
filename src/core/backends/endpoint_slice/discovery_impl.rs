//! Service discovery implementation for Kubernetes EndpointSlice
//!
//! This module implements Pingora's ServiceDiscovery trait directly for EndpointSlice,
//! allowing it to be used with Pingora's load balancing infrastructure.

use async_trait::async_trait;
use futures::FutureExt;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_load_balancing::discovery::ServiceDiscovery;
use pingora_load_balancing::selection::BackendSelection;
use pingora_load_balancing::Backend;
use pingora_load_balancing::{Backends, LoadBalancer};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, RwLock};

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

/// Wrapper for EndpointSlice that implements ServiceDiscovery
///
/// This allows using EndpointSlice directly with Pingora's load balancing.
/// Provides interior mutability for EndpointSlice updates without cloning.
/// Note: This struct is typically wrapped in Arc at the storage layer.
#[derive(Clone)]
pub struct EndpointSliceDiscovery {
    /// The EndpointSlice to discover backends from (with interior mutability)
    endpoint_slice: Arc<RwLock<EndpointSlice>>,
}

impl EndpointSliceDiscovery {
    /// Create a new EndpointSliceDiscovery from EndpointSlice
    /// Returns Arc<Self> since it's typically used with Arc at storage layer
    pub fn new(endpoint_slice: EndpointSlice) -> Self {
        Self {
            endpoint_slice: Arc::new(RwLock::new(endpoint_slice)),
        }
    }

    /// Get the port from EndpointSlice (returns first port or 80 as default)
    fn get_port(&self) -> u16 {
        let ep_slice = self.endpoint_slice.read().unwrap();
        ep_slice
            .ports
            .as_ref()
            .and_then(|ports| ports.first())
            .and_then(|p| p.port)
            .map(|p| p as u16)
            .unwrap_or(80)
    }

    /// Update the EndpointSlice data in-place
    /// This updates the EndpointSlice without replacing the entire EndpointSliceDiscovery
    pub fn update(&self, new_endpoint_slice: EndpointSlice) -> Result<(), String> {
        // Update endpoint_slice
        *self.endpoint_slice.write().unwrap() = new_endpoint_slice;

        tracing::debug!("Updated EndpointSliceDiscovery in-place");
        Ok(())
    }

    /// Execute a function with read access to the underlying EndpointSlice
    pub fn with_endpoint_slice<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&EndpointSlice) -> R,
    {
        let ep_slice = self.endpoint_slice.read().unwrap();
        f(&ep_slice)
    }

    /// Get a clone of the underlying EndpointSlice
    pub fn endpoint_slice(&self) -> EndpointSlice {
        self.endpoint_slice.read().unwrap().clone()
    }
}

#[async_trait]
impl ServiceDiscovery for EndpointSliceDiscovery {
    /// Discover backends from EndpointSlice
    ///
    /// This method is called by Pingora's load balancer to get the current
    /// list of available backends based on the EndpointSlice data.
    async fn discover(&self) -> Result<(BTreeSet<Backend>, HashMap<u64, bool>), Box<pingora_core::Error>> {
        let ep_slice = self.endpoint_slice.read().unwrap();
        let port = self.get_port();
        let backends = ep_slice.build_backends(port);

        // Return empty health map - all backends default to healthy
        let health = HashMap::new();

        Ok((backends, health))
    }
}

/// MultiEndpointSliceDiscovery aggregates multiple EndpointSlices
///
/// This is used when a Service has multiple EndpointSlices, and we need to
/// aggregate all their backends into a single load balancer.
struct MultiEndpointSliceDiscovery {
    /// Map of EndpointSlice name -> EndpointSlice
    endpoint_slices: RwLock<HashMap<String, EndpointSlice>>,
    /// Port to use for all backends
    port: u16,
}

impl MultiEndpointSliceDiscovery {
    fn new(slices: Vec<EndpointSlice>) -> Self {
        // Get port from first slice
        let port = slices
            .first()
            .and_then(|s| s.ports.as_ref()?.first()?.port)
            .map(|p| p as u16)
            .unwrap_or(8080);

        let mut slice_map = HashMap::new();
        for slice in slices {
            if let Some(name) = slice.metadata.name.clone() {
                slice_map.insert(name, slice);
            }
        }

        Self {
            endpoint_slices: RwLock::new(slice_map),
            port,
        }
    }
}

#[async_trait]
impl ServiceDiscovery for MultiEndpointSliceDiscovery {
    async fn discover(&self) -> Result<(BTreeSet<Backend>, HashMap<u64, bool>), Box<pingora_core::Error>> {
        let ep_slices = self.endpoint_slices.read().unwrap();
        let mut all_backends = BTreeSet::new();

        // Aggregate backends from all EndpointSlices
        for ep_slice in ep_slices.values() {
            let backends = ep_slice.build_backends(self.port);
            all_backends.extend(backends);
        }

        tracing::debug!(
            endpoint_slice_count = ep_slices.len(),
            backend_count = all_backends.len(),
            "Built backends from multiple EndpointSlices"
        );

        // Return empty health map - all backends default to healthy
        let health = HashMap::new();

        Ok((all_backends, health))
    }
}

/// Wrapper to make Arc<dyn ServiceDiscovery> implement ServiceDiscovery
struct DiscoveryWrapper(Arc<dyn ServiceDiscovery + Send + Sync>);

#[async_trait]
impl ServiceDiscovery for DiscoveryWrapper {
    async fn discover(&self) -> Result<(BTreeSet<Backend>, HashMap<u64, bool>), Box<pingora_core::Error>> {
        self.0.discover().await
    }
}

/// EndpointSliceLoadBalancer combines EndpointSliceDiscovery with LoadBalancer
///
/// This struct wraps an EndpointSliceDiscovery and creates a LoadBalancer that uses it
/// for service discovery. This is a generic struct that works with any BackendSelection algorithm.
pub struct EndpointSliceLoadBalancer<S>
where
    S: BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    /// The discovery implementation (either single or multi)
    #[allow(dead_code)]
    discovery: Arc<dyn ServiceDiscovery + Send + Sync>,
    /// The load balancer using the discovery
    lb: LoadBalancer<S>,
}

impl<S> EndpointSliceLoadBalancer<S>
where
    S: BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    /// Create a new EndpointSliceLoadBalancer from a single EndpointSlice
    /// Returns Arc<Self> since it's typically used with Arc at storage layer
    pub fn new(endpoint_slice: EndpointSlice) -> Arc<Self> {
        Self::new_with_slices(vec![endpoint_slice])
    }

    /// Create a new EndpointSliceLoadBalancer from multiple EndpointSlices
    /// This aggregates all EndpointSlices' backends into a single LoadBalancer
    pub fn new_with_slices(slices: Vec<EndpointSlice>) -> Arc<Self> {
        let discovery: Arc<dyn ServiceDiscovery + Send + Sync> = if slices.len() == 1 {
            // Single EndpointSlice: use the efficient single-slice discovery
            Arc::new(EndpointSliceDiscovery::new(slices.into_iter().next().unwrap()))
        } else {
            // Multiple EndpointSlices: use the aggregating multi-slice discovery
            Arc::new(MultiEndpointSliceDiscovery::new(slices))
        };

        // Clone the Arc for Backends
        let discovery_clone = discovery.clone();
        let backends = Backends::new(Box::new(DiscoveryWrapper(discovery_clone)));
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
                    tracing::debug!("LoadBalancer initialized with no backends (expected for empty EndpointSlice)");
                } else {
                    tracing::error!(
                        error = ?e,
                        "Unexpected error initializing LoadBalancer, this may cause issues"
                    );
                }
            }
            None => {
                // This should never happen for our discovery implementation
                tracing::error!("LoadBalancer update blocked - this indicates a bug in EndpointSliceDiscovery");
            }
        }

        Arc::new(Self { discovery, lb })
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
}
