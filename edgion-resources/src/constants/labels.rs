//! Kubernetes label constants

/// Standard Kubernetes labels
pub mod k8s {
    /// Service name label on EndpointSlice
    /// Used to identify which Service an EndpointSlice belongs to
    pub const SERVICE_NAME: &str = "kubernetes.io/service-name";

    /// Standard EndpointSlice `managed-by` label key.
    pub const ENDPOINTSLICE_MANAGED_BY: &str = "endpointslice.kubernetes.io/managed-by";
}

/// Labels written by the control-plane DNS resolver loop on the synthetic
/// EndpointSlices it publishes for controller-mode ExternalName Services.
pub mod dns {
    /// Marks an EndpointSlice as synthesized by the DNS resolver loop.
    pub const SYNTHETIC: &str = "edgion.io/dns-synthetic";
    /// `endpointslice.kubernetes.io/managed-by` value identifying resolver-owned
    /// slices. Also used as the Server-Side-Apply field-manager name.
    pub const MANAGED_BY_VALUE: &str = "edgion-dns-resolver";
}

/// `edgion.io/managed-by` label — provenance marker for a system-managed resource.
///
/// **This is not an enforcement mechanism, and these constants have no callers.** Nothing in the
/// workspace reads the label: no handler consults it, and the two services that write it use
/// their own local string literals rather than the values below. Keep it that way unless a
/// design explicitly introduces a reader — a value here does not grant ownership of anything.
///
/// Written in practice on `Secret` and the ACME-generated `EdgionTls`:
/// - `acme` — `edgion-controller/src/services/acme/service.rs`, `.../acme/secret_io.rs`
/// - `conf-sync-ca` — `edgion-controller/src/services/conf_sync_ca/secret_io.rs`
///   (not declared below)
///
/// It is deliberately **not** used on `EdgionConfigData`. That kind is the Center-controlled data
/// overlay with a single intended writer, so there is no ownership to arbitrate; see
/// `skills/01-architecture/05-resources/15-edgion-config-data.md` (Ownership).
pub mod managed_by {
    /// Label key
    pub const KEY: &str = "edgion.io/managed-by";

    /// Git/kubectl-authored resource. Declared for completeness; written by nothing.
    pub const OPERATOR: &str = "operator";

    /// ACME service owns the resource lifecycle (Secrets and the generated EdgionTls).
    pub const ACME: &str = "acme";

    /// Reserved for EdgionCenter-pushed data. No reader and no writer exists; the label-based
    /// ownership design that would have consumed this was rejected in favour of the CAS write
    /// model — see the skill section referenced above before reintroducing it.
    pub const CENTER: &str = "center";

    /// Reserved for a controller-regenerated body from a template ref. No reader, no writer,
    /// and no template engine.
    pub const CONTROLLER: &str = "controller";
}
