//! Service discovery implementation for Kubernetes EndpointSlice
//! 
//! This module implements Pingora's ServiceDiscovery trait directly for EndpointSlice,
//! allowing it to be used with Pingora's load balancing infrastructure.

use async_trait::async_trait;
use futures::FutureExt;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_load_balancing::discovery::ServiceDiscovery;
use pingora_load_balancing::selection::RoundRobin;
use pingora_load_balancing::{Backends, LoadBalancer};
use pingora_load_balancing::Backend;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, RwLock};
use crate::core::lb::optional_lb::OptionalLoadBalancers;

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
            let is_ready = endpoint.conditions
                .as_ref()
                .and_then(|c| c.ready)
                .unwrap_or(false);
            
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
                        weight: 1, // Default weight
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
        ep_slice.ports
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

/// EndpointSliceLoadBalancer combines EndpointSliceDiscovery with LoadBalancer
///
/// This struct wraps an EndpointSliceDiscovery and creates a LoadBalancer that uses it
/// for service discovery. This avoids the need for a Static discovery wrapper and
/// eliminates circular dependencies.
pub struct EndpointSliceLoadBalancer {
    /// The discovery implementation
    discovery: EndpointSliceDiscovery,
    /// The load balancer using the discovery (RoundRobin, always present)
    lb: Arc<LoadBalancer<RoundRobin>>,
    /// Optional load balancing algorithms (None if not needed)
    optional_lbs: Option<Arc<OptionalLoadBalancers>>,
}

impl EndpointSliceLoadBalancer {
    /// Create a new EndpointSliceLoadBalancer from EndpointSlice
    /// Returns Arc<Self> since it's typically used with Arc at storage layer
    pub fn new(endpoint_slice: EndpointSlice) -> Arc<Self> {
        let discovery = EndpointSliceDiscovery::new(endpoint_slice);
        
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
        
        // Create optional load balancers if configured
        let optional_lbs = OptionalLoadBalancers::try_new(&discovery);

        Arc::new(Self {
            discovery,
            lb: Arc::new(lb),
            optional_lbs,
        })
    }
    
    /// Update the EndpointSlice data in-place
    pub fn update(&self, new_endpoint_slice: EndpointSlice) -> Result<(), String> {
        self.discovery.update(new_endpoint_slice)
    }
    
    /// Trigger LoadBalancer update
    /// Calls lb.update() which will refresh backends from discovery
    pub async fn update_load_balancer(&self) -> Result<(), String> {
        self.lb.update()
            .await
            .map_err(|e| format!("Failed to update RoundRobin LB: {}", e))?;
        
        // Update optional algorithms if present
        if let Some(ref opts) = self.optional_lbs {
            opts.update_all().await?;
        }
        
        tracing::debug!("LoadBalancer(s) updated");
        Ok(())
    }
    
    /// Get the load balancer reference
    pub fn load_balancer(&self) -> Arc<LoadBalancer<RoundRobin>> {
        self.lb.clone()
    }
    
    /// Execute a function with read access to the underlying EndpointSlice
    pub fn with_endpoint_slice<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&EndpointSlice) -> R,
    {
        self.discovery.with_endpoint_slice(f)
    }
    
    /// Get a clone of the underlying EndpointSlice
    pub fn endpoint_slice(&self) -> EndpointSlice {
        self.discovery.endpoint_slice()
    }
    
    /// Get optional load balancers if available
    pub fn optional_lbs(&self) -> Option<&Arc<OptionalLoadBalancers>> {
        self.optional_lbs.as_ref()
    }
    
    /// Check if optional algorithms are enabled
    pub fn has_optional_algorithms(&self) -> bool {
        self.optional_lbs.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    fn create_test_endpoint_slice() -> EndpointSlice {
        EndpointSlice {
            address_type: "IPv4".to_string(),
            endpoints: vec![
                Endpoint {
                    addresses: vec!["10.0.0.1".to_string()],
                    conditions: Some(EndpointConditions {
                        ready: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                Endpoint {
                    addresses: vec!["10.0.0.2".to_string()],
                    conditions: Some(EndpointConditions {
                        ready: Some(false), // Not ready
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
            metadata: ObjectMeta {
                name: Some("test-slice".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_build_backends() {
        let ep_slice = create_test_endpoint_slice();
        let backends = ep_slice.build_backends(8080);
        
        // Should only include ready endpoint
        assert_eq!(backends.len(), 1);
        
        let backend = backends.iter().next().unwrap();
        assert_eq!(backend.addr.to_string(), "10.0.0.1:8080");
        assert_eq!(backend.weight, 1);
    }

    #[tokio::test]
    async fn test_discovery() {
        let ep_slice = create_test_endpoint_slice();
        let discovery = EndpointSliceDiscovery::new(ep_slice);
        
        let result = discovery.discover().await;
        assert!(result.is_ok());
        
        let (backends, health) = result.unwrap();
        assert_eq!(backends.len(), 1);
        
        // Health map is empty - backends default to healthy
        assert!(health.is_empty());
    }

    #[test]
    fn test_build_backends_empty() {
        let ep_slice = EndpointSlice {
            address_type: "IPv4".to_string(),
            endpoints: vec![],
            metadata: ObjectMeta {
                name: Some("empty-slice".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        
        let backends = ep_slice.build_backends(8080);
        assert!(backends.is_empty());
    }

    #[test]
    fn test_discovery_with_empty_endpoints() {
        // Test creating EndpointSliceDiscovery with no ready endpoints
        use std::collections::BTreeMap;
        let mut labels = BTreeMap::new();
        labels.insert("kubernetes.io/service-name".to_string(), "test-svc".to_string());
        
        let ep_slice = EndpointSlice {
            address_type: "IPv4".to_string(),
            endpoints: vec![],
            metadata: ObjectMeta {
                name: Some("empty-slice".to_string()),
                namespace: Some("default".to_string()),
                labels: Some(labels),
                ..Default::default()
            },
            ports: Some(vec![k8s_openapi::api::discovery::v1::EndpointPort {
                port: Some(8080),
                name: Some("http".to_string()),
                ..Default::default()
            }]),
            ..Default::default()
        };
        
        // Test if LoadBalancer::try_from_iter can handle empty backends
        println!("Testing with empty endpoints...");
        let _discovery = EndpointSliceDiscovery::new(ep_slice);
        println!("✅ EndpointSliceDiscovery created successfully with empty endpoints (no panic!)");
    }

    #[test]
    fn test_discovery_with_all_not_ready_endpoints() {
        // Test creating EndpointSliceDiscovery with endpoints that are not ready
        use std::collections::BTreeMap;
        let mut labels = BTreeMap::new();
        labels.insert("kubernetes.io/service-name".to_string(), "test-svc".to_string());
        
        let ep_slice = EndpointSlice {
            address_type: "IPv4".to_string(),
            endpoints: vec![
                k8s_openapi::api::discovery::v1::Endpoint {
                    addresses: vec!["10.0.0.1".to_string()],
                    conditions: Some(k8s_openapi::api::discovery::v1::EndpointConditions {
                        ready: Some(false),  // Not ready
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                k8s_openapi::api::discovery::v1::Endpoint {
                    addresses: vec!["10.0.0.2".to_string()],
                    conditions: Some(k8s_openapi::api::discovery::v1::EndpointConditions {
                        ready: Some(false),  // Not ready
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
            metadata: ObjectMeta {
                name: Some("not-ready-slice".to_string()),
                namespace: Some("default".to_string()),
                labels: Some(labels),
                ..Default::default()
            },
            ports: Some(vec![k8s_openapi::api::discovery::v1::EndpointPort {
                port: Some(8080),
                name: Some("http".to_string()),
                ..Default::default()
            }]),
            ..Default::default()
        };
        
        // Test if LoadBalancer can handle no ready endpoints (all not ready)
        println!("Testing with all not-ready endpoints...");
        let _discovery = EndpointSliceDiscovery::new(ep_slice);
        println!("✅ EndpointSliceDiscovery created successfully with not-ready endpoints (no panic!)");
    }

    #[test]
    fn test_build_backends_ipv6() {
        let ep_slice = EndpointSlice {
            address_type: "IPv6".to_string(),
            endpoints: vec![
                Endpoint {
                    addresses: vec!["2001:db8::1".to_string()],
                    conditions: Some(EndpointConditions {
                        ready: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                Endpoint {
                    addresses: vec!["2001:db8::2".to_string()],
                    conditions: Some(EndpointConditions {
                        ready: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
            metadata: ObjectMeta {
                name: Some("test-ipv6-slice".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        
        let backends = ep_slice.build_backends(8080);
        
        // Should include both IPv6 endpoints
        assert_eq!(backends.len(), 2);
        
        // Check that addresses are properly formatted with brackets
        let addrs: Vec<String> = backends.iter().map(|b| b.addr.to_string()).collect();
        assert!(addrs.contains(&"[2001:db8::1]:8080".to_string()));
        assert!(addrs.contains(&"[2001:db8::2]:8080".to_string()));
    }
}

