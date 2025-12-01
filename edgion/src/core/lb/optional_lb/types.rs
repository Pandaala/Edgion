//! Optional load balancing algorithm types

use std::sync::Arc;
use futures::FutureExt;
use pingora_load_balancing::selection::{BackendSelection, Random};
use pingora_load_balancing::{Backends, LoadBalancer};
use crate::core::backends::endpoint_slice::EndpointSliceDiscovery;
use crate::core::lb::optional_lb::get_policies_for_service;

/// Load balancing policy types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LbPolicy {
    /// Ketama consistent hashing
    Ketama,
    /// FNV hash-based selection  
    FnvHash,
    /// Least connection selection
    LeastConnection,
}

impl LbPolicy {
    /// Parse policy from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "ketama" | "consistent-hash" => Some(Self::Ketama),
            "fnvhash" | "fnv-hash" => Some(Self::FnvHash),
            "leastconn" | "least-connection" | "leastconnection" | "least_connection" => Some(Self::LeastConnection),
            _ => None,
        }
    }
    
    /// Get policy name
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ketama => "ketama",
            Self::FnvHash => "fnvhash",
            Self::LeastConnection => "leastconn",
        }
    }
    
    /// Parse LB policies from comma-separated string
    /// 
    /// Supports multiple aliases for each policy type:
    /// - Ketama: "ketama", "consistent-hash"
    /// - FnvHash: "fnvhash", "fnv-hash"
    /// - LeastConnection: "leastconn", "least-connection", "leastconnection", "least_connection"
    /// 
    /// # Examples
    /// ```ignore
    /// let policies = LbPolicy::parse_from_string("ketama");
    /// assert_eq!(policies, vec![LbPolicy::Ketama]);
    /// 
    /// let policies = LbPolicy::parse_from_string("ketama,fnvhash");
    /// assert_eq!(policies.len(), 2);
    /// 
    /// let policies = LbPolicy::parse_from_string("ketama, leastconnection");
    /// assert_eq!(policies.len(), 2);
    /// ```
    pub fn parse_from_string(policy_str: &str) -> Vec<Self> {
        policy_str
            .split(',')
            .filter_map(|s| {
                let trimmed = s.trim();
                match Self::from_str(trimmed) {
                    Some(policy) => Some(policy),
                    None => {
                        if !trimmed.is_empty() {
                            tracing::warn!(policy = %trimmed, "Unknown LB policy");
                        }
                        None
                    }
                }
            })
            .collect()
    }
}

/// Container for optional load balancing algorithms
/// 
/// Only requested algorithms are initialized based on Vec<LbPolicy>.
/// No locks - immutable after creation for lock-free access.
pub struct OptionalLoadBalancers {
    // Note: Using Random as placeholder for Ketama/FnvHash/LeastConnection
    // until we confirm exact Pingora 0.6 types
    pub(crate) ketama: Option<Arc<LoadBalancer<Random>>>,
    pub(crate) fnvhash: Option<Arc<LoadBalancer<Random>>>,
    pub(crate) least_conn: Option<Arc<LoadBalancer<Random>>>,
}

impl OptionalLoadBalancers {
    /// Try to create optional load balancers if needed
    /// 
    /// This is a convenience method that:
    /// 1. Queries policies via get_policies_for_service
    /// 2. Returns None if no policies configured
    /// 3. Returns None if initialization fails
    /// 4. Returns Some(Arc<...>) on success
    /// 
    /// # Arguments
    /// * `service_key` - The service key (format: "namespace/service-name")
    /// * `discovery` - The service discovery implementation
    /// 
    /// # Returns
    /// * `Option<Arc<Self>>` - The initialized load balancers or None
    pub fn try_new(service_key: &str, discovery: &EndpointSliceDiscovery) -> Option<Arc<Self>> {
        // Query policies for this service
        let policies = get_policies_for_service(service_key);
        if policies.is_empty() {
            tracing::debug!(
                service_key = %service_key,
                "No optional LB policies configured for service"
            );
            return None;
        }
        
        tracing::info!(
            service_key = %service_key,
            policies = ?policies,
            "Creating optional load balancers for service"
        );
        
        Self::new(discovery, policies).ok().map(Arc::new)
    }
    
    /// Create optional load balancers based on requested policies
    /// 
    /// # Arguments
    /// * `discovery` - The service discovery implementation
    /// * `policies` - List of algorithms to initialize
    /// 
    /// # Returns
    /// * `Ok(Self)` - Successfully initialized requested algorithms
    /// * `Err(String)` - Failed to initialize one or more algorithms
    pub fn new(
        discovery: &EndpointSliceDiscovery,
        policies: Vec<LbPolicy>,
    ) -> Result<Self, String> {
        let mut ketama = None;
        let mut fnvhash = None;
        let mut least_conn = None;
        
        for policy in &policies {
            match policy {
                LbPolicy::Ketama => {
                    ketama = Some(Self::init_lb(discovery, "Ketama")?);
                    tracing::debug!("Ketama LoadBalancer initialized");
                }
                LbPolicy::FnvHash => {
                    fnvhash = Some(Self::init_lb(discovery, "FnvHash")?);
                    tracing::debug!("FnvHash LoadBalancer initialized");
                }
                LbPolicy::LeastConnection => {
                    least_conn = Some(Self::init_lb(discovery, "LeastConnection")?);
                    tracing::debug!("LeastConnection LoadBalancer initialized");
                }
            }
        }
        
        tracing::info!(
            policies = ?policies,
            "OptionalLoadBalancers initialized"
        );
        
        Ok(Self {
            ketama,
            fnvhash,
            least_conn,
        })
    }
    
    /// Helper to initialize a load balancer
    pub(crate) fn init_lb<S>(
        discovery: &EndpointSliceDiscovery,
        name: &str,
    ) -> Result<Arc<LoadBalancer<S>>, String> 
    where
        S: BackendSelection + 'static,
        S::Iter: pingora_load_balancing::selection::BackendIter,
    {
        let backends = Backends::new(Box::new(discovery.clone()));
        let lb = LoadBalancer::from_backends(backends);
        
        lb.update()
            .now_or_never()
            .ok_or_else(|| format!("{} LB update blocked", name))?
            .map_err(|e| format!("Failed to init {} LB: {:?}", name, e))?;
        
        Ok(Arc::new(lb))
    }
    
    /// Update all initialized load balancers
    pub async fn update_all(&self) -> Result<(), String> {
        if let Some(ref lb) = self.ketama {
            lb.update().await
                .map_err(|e| format!("Ketama update failed: {:?}", e))?;
        }
        
        if let Some(ref lb) = self.fnvhash {
            lb.update().await
                .map_err(|e| format!("FnvHash update failed: {:?}", e))?;
        }
        
        if let Some(ref lb) = self.least_conn {
            lb.update().await
                .map_err(|e| format!("LeastConnection update failed: {:?}", e))?;
        }
        
        Ok(())
    }
    
    /// Get Ketama load balancer if initialized
    pub fn ketama(&self) -> Option<Arc<LoadBalancer<Random>>> {
        self.ketama.clone()
    }
    
    /// Get FnvHash load balancer if initialized
    pub fn fnvhash(&self) -> Option<Arc<LoadBalancer<Random>>> {
        self.fnvhash.clone()
    }
    
    /// Get LeastConnection load balancer if initialized
    pub fn least_conn(&self) -> Option<Arc<LoadBalancer<Random>>> {
        self.least_conn.clone()
    }
    
    /// Check which policies are initialized
    pub fn initialized_policies(&self) -> Vec<LbPolicy> {
        let mut policies = Vec::new();
        if self.ketama.is_some() {
            policies.push(LbPolicy::Ketama);
        }
        if self.fnvhash.is_some() {
            policies.push(LbPolicy::FnvHash);
        }
        if self.least_conn.is_some() {
            policies.push(LbPolicy::LeastConnection);
        }
        policies
    }
}

