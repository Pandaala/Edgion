//! Service discovery implementation for Kubernetes EndpointSlice
//! 
//! This module implements Pingora's ServiceDiscovery trait directly for EndpointSlice,
//! allowing it to be used with Pingora's load balancing infrastructure.

use async_trait::async_trait;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_load_balancing::discovery::ServiceDiscovery;
use pingora_load_balancing::selection::RoundRobin;
use pingora_load_balancing::{Backends, LoadBalancer};
use pingora_load_balancing::Backend;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, RwLock, Weak};

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
/// Uses Arc<RwLock<>> for interior mutability to allow in-place updates.
pub struct EndpointSliceDiscovery {
    /// The EndpointSlice to discover backends from (with interior mutability)
    endpoint_slice: Arc<RwLock<EndpointSlice>>,
    /// Default port to use for backend connections (from first port in EndpointSlice)
    default_port: Arc<RwLock<u16>>,
    /// Load balancer with RoundRobin selection (with interior mutability)
    /// Uses Option to allow delayed initialization
    lb: Arc<RwLock<Option<LoadBalancer<RoundRobin>>>>,
    /// Weak self-reference for service discovery
    _self: Weak<Self>,
}

// Manual Clone implementation
impl Clone for EndpointSliceDiscovery {
    fn clone(&self) -> Self {
        Self {
            endpoint_slice: self.endpoint_slice.clone(),
            default_port: self.default_port.clone(),
            lb: self.lb.clone(),
            _self: self._self.clone(),
        }
    }
}

impl EndpointSliceDiscovery {
    /// Create a new EndpointSliceDiscovery with explicit port
    pub fn new(endpoint_slice: EndpointSlice, default_port: u16) -> Arc<Self> {
        // Create Arc with None lb first
        let discovery = Arc::new_cyclic(|weak| Self {
            endpoint_slice: Arc::new(RwLock::new(endpoint_slice)),
            default_port: Arc::new(RwLock::new(default_port)),
            lb: Arc::new(RwLock::new(None)),
            _self: weak.clone(),
        });
        
        // Now create LoadBalancer using this discovery
        // We need to box it as dyn ServiceDiscovery
        let discovery_boxed: Box<dyn ServiceDiscovery + Send + Sync> = Box::new(discovery.as_ref().clone());
        let backends = Backends::new(discovery_boxed);
        let lb = LoadBalancer::from_backends(backends);
        
        // Store the LoadBalancer
        *discovery.lb.write().unwrap() = Some(lb);
        
        discovery
    }
    
    /// Create a new EndpointSliceDiscovery using the first port from EndpointSlice as default
    /// Returns error if no ports are defined
    pub fn from_endpoint_slice(endpoint_slice: EndpointSlice) -> Result<Arc<Self>, String> {
        // Get the first port as default port, return error if no ports defined
        let default_port = endpoint_slice.ports
            .as_ref()
            .and_then(|ports| ports.first())
            .and_then(|p| p.port)
            .ok_or_else(|| "No port defined in EndpointSlice".to_string())?;
        
        let default_port = default_port as u16;
        
        Ok(Self::new(endpoint_slice, default_port))
    }
    
    /// Update the EndpointSlice data in-place
    /// This updates the EndpointSlice without replacing the entire EndpointSliceDiscovery
    pub fn update(&self, new_endpoint_slice: EndpointSlice) -> Result<(), String> {
        // Get or keep the default port
        let port = {
            let current_port = *self.default_port.read().unwrap();
            // Try to get port from new endpoint slice, fallback to current port
            new_endpoint_slice.ports
                .as_ref()
                .and_then(|ports| ports.first())
                .and_then(|p| p.port)
                .map(|p| p as u16)
                .unwrap_or(current_port)
        };
        
        // Update endpoint_slice and port
        *self.endpoint_slice.write().unwrap() = new_endpoint_slice;
        *self.default_port.write().unwrap() = port;
        
        tracing::debug!("Updated EndpointSliceDiscovery in-place");
        Ok(())
    }
    
    /// Trigger LoadBalancer update
    /// This will call lb.update() which internally calls self.discover()
    /// to rebuild backends from the updated EndpointSlice
    /// Note: This is an async method
    pub async fn update_load_balancer(&self) -> Result<(), String> {
        let lb_arc = self.lb.clone();
        
        // Get LoadBalancer without holding lock during async call
        let lb_guard = lb_arc.read().unwrap();
        if let Some(_) = lb_guard.as_ref() {
            // Release the lock before calling async update
            drop(lb_guard);
            
            // Re-acquire to call update
            let lb_guard = lb_arc.read().unwrap();
            if let Some(lb) = lb_guard.as_ref() {
                lb.update()
                    .await
                    .map_err(|e| format!("Failed to update LoadBalancer: {}", e))?;
                
                tracing::debug!("LoadBalancer updated via discover()");
            }
        }
        
        Ok(())
    }
    
    /// Get a reference to the load balancer (returns Arc for shared access)
    pub fn load_balancer(&self) -> Option<Arc<RwLock<Option<LoadBalancer<RoundRobin>>>>> {
        Some(self.lb.clone())
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
    
    /// Get service key from this EndpointSlice
    /// Returns "namespace/service-name" based on the kubernetes.io/service-name label
    pub fn service_key(&self) -> Option<String> {
        const SERVICE_NAME_LABEL: &str = "kubernetes.io/service-name";
        self.with_endpoint_slice(|ep_slice| {
            let metadata = &ep_slice.metadata;
            let namespace = metadata.namespace.as_deref()?;
            let labels = metadata.labels.as_ref()?;
            let service_name = labels.get(SERVICE_NAME_LABEL)?;
            Some(format!("{}/{}", namespace, service_name))
        })
    }
    
    /// Get the default port
    pub fn default_port(&self) -> u16 {
        *self.default_port.read().unwrap()
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
        let port = *self.default_port.read().unwrap();
        let backends = ep_slice.build_backends(port);
        
        // Return empty health map - all backends default to healthy
        let health = HashMap::new();
        
        Ok((backends, health))
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
        let discovery = EndpointSliceDiscovery::new(ep_slice, 8080);
        
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

