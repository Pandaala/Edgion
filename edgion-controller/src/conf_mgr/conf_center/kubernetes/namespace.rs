//! Namespace watch mode configuration for Kubernetes controller

/// Namespace watch mode for the controller
#[derive(Debug, Clone)]
pub enum NamespaceWatchMode {
    /// Watch all namespaces (cluster-wide)
    AllNamespaces,
    /// Watch a single namespace
    SingleNamespace(String),
    /// Watch multiple specific namespaces
    MultipleNamespaces(Vec<String>),
}

impl NamespaceWatchMode {
    /// Create from a list of namespaces
    pub fn from_namespaces(mut namespaces: Vec<String>) -> Self {
        namespaces.sort();
        namespaces.dedup();
        match namespaces.len() {
            0 => Self::AllNamespaces,
            1 => Self::SingleNamespace(namespaces.into_iter().next().unwrap()),
            _ => Self::MultipleNamespaces(namespaces),
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/conf_center/kubernetes/namespace_test.rs"]
mod tests;
