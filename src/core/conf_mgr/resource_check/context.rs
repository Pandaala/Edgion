//! Resource Check Context
//!
//! Provides context for checking resource dependencies by reading from ConfigServer cache.

use crate::core::conf_sync::ConfigServer;

/// Context for resource validation checks
///
/// Wraps ConfigServer to provide convenient methods for checking
/// resource existence and other validation needs.
pub struct ResourceCheckContext<'a> {
    config_server: &'a ConfigServer,
}

impl<'a> ResourceCheckContext<'a> {
    /// Create a new ResourceCheckContext
    pub fn new(config_server: &'a ConfigServer) -> Self {
        Self { config_server }
    }

    /// Check if a Gateway exists in the cache
    ///
    /// # Arguments
    /// * `namespace` - The namespace to check (None for cluster-scoped or default)
    /// * `name` - The Gateway name
    pub fn gateway_exists(&self, namespace: Option<&str>, name: &str) -> bool {
        let gateways = self.config_server.gateways.list_owned();
        gateways.data.iter().any(|gw| {
            let gw_name_matches = gw.metadata.name.as_deref() == Some(name);
            let gw_namespace_matches = match (namespace, gw.metadata.namespace.as_deref()) {
                (Some(ns), Some(gw_ns)) => ns == gw_ns,
                (None, None) => true,
                (None, Some(_)) => true, // If no namespace specified, match any
                _ => false,
            };
            gw_name_matches && gw_namespace_matches
        })
    }

    /// Check if a Secret exists in the cache
    ///
    /// # Arguments
    /// * `namespace` - The namespace to check
    /// * `name` - The Secret name
    pub fn secret_exists(&self, namespace: Option<&str>, name: &str) -> bool {
        let secrets = self.config_server.secrets.list_owned();
        secrets.data.iter().any(|s| {
            let s_name_matches = s.metadata.name.as_deref() == Some(name);
            let s_namespace_matches = match (namespace, s.metadata.namespace.as_deref()) {
                (Some(ns), Some(s_ns)) => ns == s_ns,
                (None, None) => true,
                (None, Some(_)) => true,
                _ => false,
            };
            s_name_matches && s_namespace_matches
        })
    }

    /// Get the underlying ConfigServer reference
    pub fn config_server(&self) -> &'a ConfigServer {
        self.config_server
    }
}
