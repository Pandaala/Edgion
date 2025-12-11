//! Hidden logic structures for internal processing
//! These structures are not serialized, only used for runtime analysis

use std::sync::Arc;
use crate::core::filters::PluginRuntime;
use super::{HTTPRoute, HTTPRouteFilterType, LocalObjectReference};

/// Hash source type for consistent hash LB policy
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsistentHashOn {
    /// Hash based on request header
    Header(String),
    /// Hash based on cookie
    Cookie(String),
    /// Hash based on query argument
    Arg(String),
}

/// Parsed LB policy from extensionRef
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedLBPolicy {
    /// Consistent hash with specified source
    ConsistentHash(ConsistentHashOn),
    /// Least connection
    LeastConn,
}

/// Parsed extension info attached to HTTPBackendRef
/// This is computed at runtime, not from YAML
#[derive(Debug, Clone, Default)]
pub struct BackendExtensionInfo {
    /// Parsed LB policy if extensionRef specifies one
    pub lb_policy: Option<ParsedLBPolicy>,
}

impl BackendExtensionInfo {
    /// Parse from LocalObjectReference
    pub fn from_extension_ref(ext_ref: &LocalObjectReference) -> Self {
        let lb_policy = Self::parse_lb_policy(ext_ref);
        Self { lb_policy }
    }
    
    /// Parse LB policy from extensionRef
    /// 
    /// Supported formats:
    /// - group: edgion.io, kind: LBPolicyConsistentHash, name: header.x-user-id
    /// - group: edgion.io, kind: LBPolicyConsistentHash, name: cookie.session-id
    /// - group: edgion.io, kind: LBPolicyConsistentHash, name: arg.user-id
    /// - group: edgion.io, kind: LBPolicyLeastConn, name: default
    fn parse_lb_policy(ext_ref: &LocalObjectReference) -> Option<ParsedLBPolicy> {
        // Check group (empty string means core API group, otherwise should be edgion.io)
        if !ext_ref.group.is_empty() && ext_ref.group != "edgion.io" {
                return None;
        }
        
        match ext_ref.kind.as_str() {
            "LBPolicyConsistentHash" => {
                Self::parse_consistent_hash_name(&ext_ref.name)
                    .map(ParsedLBPolicy::ConsistentHash)
            }
            "LBPolicyLeastConn" => {
                Some(ParsedLBPolicy::LeastConn)
            }
            _ => None,
        }
    }
    
    /// Parse consistent hash source from name
    /// Format: "header.xxx" / "cookie.xxx" / "arg.xxx"
    fn parse_consistent_hash_name(name: &str) -> Option<ConsistentHashOn> {
        let parts: Vec<&str> = name.splitn(2, '.').collect();
        match parts.as_slice() {
            ["header", key] => Some(ConsistentHashOn::Header(key.to_string())),
            ["cookie", key] => Some(ConsistentHashOn::Cookie(key.to_string())),
            ["arg", key] => Some(ConsistentHashOn::Arg(key.to_string())),
            _ => None,
        }
    }
}

/// Extension trait for HTTPRoute to parse hidden logic
impl HTTPRoute {
    /// Parse all extension_ref in backend_refs and populate extension_info fields
    /// 
    /// This method should be called after deserializing HTTPRoute from YAML/JSON
    /// to populate the runtime-only extension_info fields.
    /// 
    /// # Example
    /// ```ignore
    /// let mut route: HTTPRoute = serde_yaml::from_str(yaml_str)?;
    /// route.parse_hidden_logic();
    /// ```
    pub fn parse_hidden_logic(&mut self) {
        let Some(rules) = self.spec.rules.as_mut() else {
            return;
        };

        // Get namespace for ExtensionRef lookups
        let namespace = self.metadata.namespace.as_deref().unwrap_or("default");
        
        for rule in rules.iter_mut() {
            // Initialize rule-level plugin_runtime from rule.filters
            if let Some(filters) = &rule.filters {
                rule.plugin_runtime = Arc::new(PluginRuntime::from_httproute_filters(filters, namespace));
            }

            let Some(backend_refs) = rule.backend_refs.as_mut() else {
                continue;
            };
            
            for backend_ref in backend_refs.iter_mut() {
                // Find ExtensionRef filter in backend_ref.filters
                let extension_info = backend_ref.filters.as_ref()
                    .and_then(|filters| {
                        filters.iter()
                            .find(|f| f.filter_type == HTTPRouteFilterType::ExtensionRef)
                            .and_then(|f| f.extension_ref.as_ref())
                            .map(BackendExtensionInfo::from_extension_ref)
                    })
                    .unwrap_or_default();
                
                backend_ref.extension_info = extension_info;

                // Initialize plugin_runtime from filters
                if let Some(filters) = &backend_ref.filters {
                    backend_ref.plugin_runtime = Arc::new(PluginRuntime::from_httproute_filters(filters, namespace));
                }
            }
        }
    }
    
    /// Parse and pre-process timeout configurations for all rules
    /// 
    /// This method is called during route loading (in pre_parse) to avoid runtime parsing overhead.
    /// It parses timeout strings into Duration objects and stores them in rule.parsed_timeouts.
    pub fn parse_timeouts(&mut self) {
        let Some(rules) = self.spec.rules.as_mut() else {
            return;
        };
        
        for rule in rules.iter_mut() {
            // Parse timeouts for each rule
            if let Some(timeouts) = &rule.timeouts {
                rule.parsed_timeouts = crate::types::resources::http_route::ParsedRouteTimeouts::from_config(timeouts);
                
                if rule.parsed_timeouts.is_some() {
                    tracing::debug!(
                        "Parsed route-level timeouts for HTTPRoute rule"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_consistent_hash_header() {
        let ext_ref = LocalObjectReference {
            group: "edgion.io".to_string(),
            kind: "LBPolicyConsistentHash".to_string(),
            name: "header.x-user-id".to_string(),
        };
        
        let info = BackendExtensionInfo::from_extension_ref(&ext_ref);
        assert_eq!(
            info.lb_policy,
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Header("x-user-id".to_string())))
        );
    }

    #[test]
    fn test_parse_consistent_hash_cookie() {
        let ext_ref = LocalObjectReference {
            group: "edgion.io".to_string(),
            kind: "LBPolicyConsistentHash".to_string(),
            name: "cookie.session-id".to_string(),
        };
        
        let info = BackendExtensionInfo::from_extension_ref(&ext_ref);
        assert_eq!(
            info.lb_policy,
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Cookie("session-id".to_string())))
        );
    }

    #[test]
    fn test_parse_consistent_hash_arg() {
        let ext_ref = LocalObjectReference {
            group: "edgion.io".to_string(),
            kind: "LBPolicyConsistentHash".to_string(),
            name: "arg.user-id".to_string(),
        };
        
        let info = BackendExtensionInfo::from_extension_ref(&ext_ref);
        assert_eq!(
            info.lb_policy,
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Arg("user-id".to_string())))
        );
    }

    #[test]
    fn test_parse_least_conn() {
        let ext_ref = LocalObjectReference {
            group: "edgion.io".to_string(),
            kind: "LBPolicyLeastConn".to_string(),
            name: "default".to_string(),
        };
        
        let info = BackendExtensionInfo::from_extension_ref(&ext_ref);
        assert_eq!(info.lb_policy, Some(ParsedLBPolicy::LeastConn));
    }

    #[test]
    fn test_parse_unknown_kind() {
        let ext_ref = LocalObjectReference {
            group: "edgion.io".to_string(),
            kind: "UnknownPolicy".to_string(),
            name: "default".to_string(),
        };
        
        let info = BackendExtensionInfo::from_extension_ref(&ext_ref);
        assert_eq!(info.lb_policy, None);
    }

    #[test]
    fn test_parse_wrong_group() {
        let ext_ref = LocalObjectReference {
            group: "other.io".to_string(),
            kind: "LBPolicyConsistentHash".to_string(),
            name: "header.x-user-id".to_string(),
        };
        
        let info = BackendExtensionInfo::from_extension_ref(&ext_ref);
        assert_eq!(info.lb_policy, None);
    }

    #[test]
    fn test_parse_empty_group() {
        // Empty group (core API group), should still work
        let ext_ref = LocalObjectReference {
            group: String::new(),
            kind: "LBPolicyConsistentHash".to_string(),
            name: "header.x-user-id".to_string(),
        };
        
        let info = BackendExtensionInfo::from_extension_ref(&ext_ref);
        assert_eq!(
            info.lb_policy,
            Some(ParsedLBPolicy::ConsistentHash(ConsistentHashOn::Header("x-user-id".to_string())))
        );
    }
}

