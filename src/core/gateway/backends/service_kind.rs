/// EdgionService defines the types of backend services
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
    fn test_edgion_service_from_kind_default() {
        assert_eq!(EdgionService::from_kind(None), EdgionService::Service);
        assert_eq!(EdgionService::from_kind(Some(&String::new())), EdgionService::Service);
    }

    #[test]
    fn test_edgion_service_from_kind_standard() {
        assert_eq!(
            EdgionService::from_kind(Some(&"Service".to_string())),
            EdgionService::Service
        );
        assert_eq!(
            EdgionService::from_kind(Some(&"ServiceClusterIp".to_string())),
            EdgionService::ServiceClusterIp
        );
        assert_eq!(
            EdgionService::from_kind(Some(&"ServiceImport".to_string())),
            EdgionService::ServiceImport
        );
        assert_eq!(
            EdgionService::from_kind(Some(&"ServiceExternalName".to_string())),
            EdgionService::ServiceExternalName
        );
    }

    #[test]
    fn test_edgion_service_from_kind_explicit_endpoint_slice() {
        assert_eq!(
            EdgionService::from_kind(Some(&"ServiceEndpointSlice".to_string())),
            EdgionService::ServiceEndpointSlice
        );
        assert_eq!(
            EdgionService::from_kind(Some(&"EndpointSlice".to_string())),
            EdgionService::ServiceEndpointSlice
        );
    }

    #[test]
    fn test_edgion_service_from_kind_unknown() {
        assert_eq!(
            EdgionService::from_kind(Some(&"UnknownKind".to_string())),
            EdgionService::Unsupported("UnknownKind".to_string())
        );
        assert_eq!(
            EdgionService::from_kind(Some(&"RandomString".to_string())),
            EdgionService::Unsupported("RandomString".to_string())
        );
        // Legacy endpoint kinds are now unsupported
        assert_eq!(
            EdgionService::from_kind(Some(&"ServiceEndpoint".to_string())),
            EdgionService::Unsupported("ServiceEndpoint".to_string())
        );
        assert_eq!(
            EdgionService::from_kind(Some(&"Endpoint".to_string())),
            EdgionService::Unsupported("Endpoint".to_string())
        );
    }
}
