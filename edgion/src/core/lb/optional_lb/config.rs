//! Configuration query for optional load balancing algorithms

use super::types::LbPolicy;

/// Get list of LB policies for a service from configuration
/// 
/// This function queries configuration sources to determine which
/// optional algorithms should be enabled for the service.
/// 
/// Configuration sources (priority order):
/// 1. Service annotation (e.g., `edgion.io/lb-algorithms: "ketama,fnvhash"`)
/// 2. Global configuration (EdgionGatewayConfig)
/// 3. Default: empty Vec (only RoundRobin)
/// 
/// # Arguments
/// * `service_key` - The service key (format: "namespace/service")
/// 
/// # Returns
/// * `Vec<LbPolicy>` - List of policies to initialize (empty = no optional LBs)
pub fn get_policies_for_service(service_key: &str) -> Vec<LbPolicy> {
    // TODO: Implement actual configuration query
    // 
    // Implementation ideas:
    // 1. Query Service annotation: "edgion.io/lb-algorithms: ketama,fnvhash"
    //    ```rust
    //    let svc_store = get_global_service_store();
    //    if let Some(service) = svc_store.get(service_key) {
    //        if let Some(annotations) = service.metadata.annotations.as_ref() {
    //            if let Some(algos) = annotations.get("edgion.io/lb-algorithms") {
    //                return parse_policies(algos);
    //            }
    //        }
    //    }
    //    ```
    // 
    // 2. Query global configuration
    //    ```rust
    //    let config = get_global_gateway_config();
    //    config.load_balancing.default_optional_policies.clone()
    //    ```
    // 
    // 3. Support wildcard rules (e.g., "production/*" namespace)
    
    // Current: default to empty (only RoundRobin)
    tracing::debug!(
        service_key = %service_key,
        "Checking optional LB policies (current: always empty)"
    );
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

