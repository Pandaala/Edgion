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
mod tests {
    use super::*;
    #[test]
    fn configured_namespace_sources_are_fixed_and_deduplicated() {
        assert!(matches!(
            NamespaceWatchMode::from_namespaces(Vec::new()),
            NamespaceWatchMode::AllNamespaces
        ));
        assert!(
            matches!(NamespaceWatchMode::from_namespaces(vec!["a".into(),"a".into()]),NamespaceWatchMode::SingleNamespace(ns) if ns=="a")
        );
        assert!(
            matches!(NamespaceWatchMode::from_namespaces(vec!["b".into(),"a".into(),"b".into()]),NamespaceWatchMode::MultipleNamespaces(ns) if ns==vec!["a","b"])
        );
    }
}
