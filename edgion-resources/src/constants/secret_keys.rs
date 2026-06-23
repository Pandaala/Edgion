//! Secret data key constants
//!
//! These constants define the standard keys used in Kubernetes Secret data
//! for TLS certificates and other sensitive information.

/// TLS certificate keys (standard Kubernetes TLS Secret format)
pub mod tls {
    /// TLS certificate in PEM format
    pub const CERT: &str = "tls.crt";
    /// TLS private key in PEM format
    pub const KEY: &str = "tls.key";
    /// CA certificate for mTLS verification
    pub const CA_CERT: &str = "ca.crt";
}
