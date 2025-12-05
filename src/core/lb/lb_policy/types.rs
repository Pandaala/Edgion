//! Optional load balancing algorithm types

/// Load balancing policy types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LbPolicy {
    /// Consistent hashing
    Consistent,
    /// Least connection selection
    LeastConnection,
}

impl LbPolicy {
    /// Parse policy from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "consistent" | "consistent-hash" | "ketama" => Some(Self::Consistent),
            "leastconn" | "least-connection" | "leastconnection" | "least_connection" => Some(Self::LeastConnection),
            _ => None,
        }
    }
    
    /// Get policy name
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Consistent => "consistent",
            Self::LeastConnection => "leastconn",
        }
    }
    
    /// Parse LB policies from comma-separated string
    /// 
    /// Supports multiple aliases for each policy type:
    /// - Consistent: "consistent", "consistent-hash", "ketama" (兼容旧配置)
    /// - LeastConnection: "leastconn", "least-connection", "leastconnection", "least_connection"
    /// 
    /// # Examples
    /// ```ignore
    /// let policies = LbPolicy::parse_from_string("consistent");
    /// assert_eq!(policies, vec![LbPolicy::Consistent]);
    /// 
    /// let policies = LbPolicy::parse_from_string("consistent,leastconn");
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
