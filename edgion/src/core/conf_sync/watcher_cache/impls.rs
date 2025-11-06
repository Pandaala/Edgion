use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::core::ObjectMeta;

use super::traits::Versionable;
use crate::types::{EdgionTls, Gateway, HTTPRoute};

/// Helper function to extract version from Kubernetes resource_version string
/// Returns 0 if resource_version is None or cannot be parsed
fn extract_version(metadata: &ObjectMeta) -> u64 {
    metadata
        .resource_version
        .as_ref()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
}

impl Versionable for Gateway {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
}

impl Versionable for HTTPRoute {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
}

impl Versionable for Service {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
}

impl Versionable for EndpointSlice {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
}

impl Versionable for Secret {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
}

impl Versionable for EdgionTls {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
}

impl Versionable for String {
    fn get_version(&self) -> u64 {
        // For GatewayClass (which is just a String), we use hash or a fixed version
        // Since GatewayClass is just a string identifier, we'll use 0
        0
    }
}

