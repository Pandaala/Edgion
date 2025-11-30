//! Configuration query for optional load balancing algorithms

use super::types::LbPolicy;

/// Get list of LB policies for a service from configuration
/// 
/// This function queries global configuration to determine which
/// optional algorithms should be enabled.
/// 
/// Configuration sources (priority order):
/// 1. Global configuration (EdgionGatewayConfig)
/// 2. Environment variables
/// 3. Default: empty Vec (only RoundRobin)
/// 
/// # Returns
/// * `Vec<LbPolicy>` - List of policies to initialize (empty = no optional LBs)
pub fn get_policies_for_service() -> Vec<LbPolicy> {
    // TODO: Implement actual configuration query
    // 
    // Implementation ideas:
    // 1. Query global configuration
    //    ```rust
    //    let config = get_global_gateway_config();
    //    if config.load_balancing.enable_optional_algorithms {
    //        return parse_policies(&config.load_balancing.algorithms);
    //    }
    //    ```
    // 
    // 2. Query environment variables
    //    ```rust
    //    if let Ok(algos) = std::env::var("EDGION_LB_ALGORITHMS") {
    //        return parse_policies(&algos);
    //    }
    //    ```
    // 
    // 3. Feature flags or dynamic config from etcd
    
    // Current: default to empty (only RoundRobin)
    Vec::new()
}

/// Parse policy list from string (e.g., "ketama,fnvhash,leastconn")
#[allow(dead_code)]
pub fn parse_policies(s: &str) -> Vec<LbPolicy> {
    s.split(',')
        .filter_map(|part| LbPolicy::from_str(part.trim()))
        .collect()
}

/// Parse service key into namespace and name
#[allow(dead_code)]
fn parse_service_key(service_key: &str) -> Option<(&str, &str)> {
    let parts: Vec<&str> = service_key.split('/').collect();
    if parts.len() == 2 {
        Some((parts[0], parts[1]))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_policies() {
        let policies = parse_policies("ketama,fnvhash");
        assert_eq!(policies.len(), 2);
        assert!(policies.contains(&LbPolicy::Ketama));
        assert!(policies.contains(&LbPolicy::FnvHash));
    }
    
    #[test]
    fn test_parse_policies_with_spaces() {
        let policies = parse_policies("ketama, fnvhash , leastconn");
        assert_eq!(policies.len(), 3);
    }
    
    #[test]
    fn test_parse_policies_invalid() {
        let policies = parse_policies("invalid,ketama");
        assert_eq!(policies.len(), 1);
        assert!(policies.contains(&LbPolicy::Ketama));
    }
    
    #[test]
    fn test_parse_service_key() {
        assert_eq!(
            parse_service_key("default/my-service"),
            Some(("default", "my-service"))
        );
        assert_eq!(
            parse_service_key("namespace-with-dash/service-name"),
            Some(("namespace-with-dash", "service-name"))
        );
    }
    
    #[test]
    fn test_parse_service_key_invalid() {
        assert_eq!(parse_service_key("invalid"), None);
        assert_eq!(parse_service_key("too/many/parts"), None);
    }
}

