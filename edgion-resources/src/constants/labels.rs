//! Kubernetes label constants

/// Standard Kubernetes labels
pub mod k8s {
    /// Service name label on EndpointSlice
    /// Used to identify which Service an EndpointSlice belongs to
    pub const SERVICE_NAME: &str = "kubernetes.io/service-name";
}

/// `edgion.io/managed-by` label — declares the lifecycle owner of a resource.
///
/// Used on `EdgionData` (and `Secret`/`EdgionTls` managed by ACME).
/// The label controls how the controller treats the spec on reconcile:
/// - `operator` (default): git/kubectl authoritative; controller never mutates spec.
/// - `acme`: ACME service manages the certificate lifecycle (existing usage).
/// - `center`: placeholder object; real data delivered via fed_sync from EdgionCenter
///   — controller does not overwrite spec from git.
///   **DESIGN-DECLARED, NOT YET IMPLEMENTED** — pending EdgionCenter coordination.
/// - `controller`: controller regenerates body from a template ref.
///   **DESIGN-DECLARED, NOT YET IMPLEMENTED** — pending template-engine design.
pub mod managed_by {
    /// Label key
    pub const KEY: &str = "edgion.io/managed-by";

    /// Default ownership: git/kubectl authoritative; controller never mutates spec.
    /// This is the only actively reconciled mode today.
    pub const OPERATOR: &str = "operator";

    /// ACME service owns the resource lifecycle (existing usage on Secrets and EdgionTls).
    pub const ACME: &str = "acme";

    /// Placeholder: real data is pushed via fed_sync from EdgionCenter.
    /// Controller does not overwrite spec from git.
    /// **NOT YET IMPLEMENTED** — requires EdgionCenter coordination (I1).
    pub const CENTER: &str = "center";

    /// Controller regenerates spec body from a template ref.
    /// **NOT YET IMPLEMENTED** — requires template-engine design.
    pub const CONTROLLER: &str = "controller";
}
