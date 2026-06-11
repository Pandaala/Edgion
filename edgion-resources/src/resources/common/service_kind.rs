/// Backend service kind — pure classifier, no runtime state.
///
/// Matches a `kind` string from a BackendRef into one of the known
/// service variants. Gateway-side code re-exports this from types so
/// all consumers share a single definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgionService {
    /// Standard Kubernetes Service (default behavior, uses EndpointSlice)
    Service,
    /// Service with ClusterIP (direct connection, no LB)
    ServiceClusterIp,
    /// ServiceImport for multi-cluster
    ServiceImport,
    /// Service with ExternalName
    ServiceExternalName,
    /// Force use EndpointSlice resource for backend discovery
    ServiceEndpointSlice,
    /// Unsupported backend kind (Gateway API conformance: must return 500)
    Unsupported(String),
}

impl EdgionService {
    /// Parse service type from kind string
    pub fn from_kind(kind: Option<&String>) -> Self {
        match kind {
            None => EdgionService::Service,
            Some(k) if k.is_empty() => EdgionService::Service,
            Some(k) => match k.as_str() {
                "Service" => EdgionService::Service,
                "ServiceClusterIp" => EdgionService::ServiceClusterIp,
                "ServiceImport" => EdgionService::ServiceImport,
                "ServiceExternalName" => EdgionService::ServiceExternalName,
                "ServiceEndpointSlice" | "EndpointSlice" => EdgionService::ServiceEndpointSlice,
                _ => EdgionService::Unsupported(k.clone()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_edgion_service_from_kind_table() {
        let cases: &[(&str, Option<String>, EdgionService)] = &[
            ("default_none", None, EdgionService::Service),
            ("default_empty", Some(String::new()), EdgionService::Service),
            ("standard_service", Some("Service".to_string()), EdgionService::Service),
            (
                "standard_cluster_ip",
                Some("ServiceClusterIp".to_string()),
                EdgionService::ServiceClusterIp,
            ),
            (
                "standard_import",
                Some("ServiceImport".to_string()),
                EdgionService::ServiceImport,
            ),
            (
                "standard_external_name",
                Some("ServiceExternalName".to_string()),
                EdgionService::ServiceExternalName,
            ),
            (
                "endpoint_slice_long",
                Some("ServiceEndpointSlice".to_string()),
                EdgionService::ServiceEndpointSlice,
            ),
            (
                "endpoint_slice_short",
                Some("EndpointSlice".to_string()),
                EdgionService::ServiceEndpointSlice,
            ),
            (
                "unknown_kind",
                Some("UnknownKind".to_string()),
                EdgionService::Unsupported("UnknownKind".to_string()),
            ),
            (
                "unknown_random",
                Some("RandomString".to_string()),
                EdgionService::Unsupported("RandomString".to_string()),
            ),
            (
                "legacy_service_endpoint",
                Some("ServiceEndpoint".to_string()),
                EdgionService::Unsupported("ServiceEndpoint".to_string()),
            ),
            (
                "legacy_endpoint",
                Some("Endpoint".to_string()),
                EdgionService::Unsupported("Endpoint".to_string()),
            ),
        ];

        for (label, input, expected) in cases {
            assert_eq!(
                EdgionService::from_kind(input.as_ref()),
                *expected,
                "case '{}' failed",
                label
            );
        }
    }
}
