//! Kubernetes label constants

/// Standard Kubernetes labels
pub mod k8s {
    /// Service name label on EndpointSlice
    /// Used to identify which Service an EndpointSlice belongs to
    pub const SERVICE_NAME: &str = "kubernetes.io/service-name";
}
