pub mod runtime;
pub mod store;
pub mod validation;

// Backend-specific modules
#[cfg(feature = "boringssl")]
pub mod boringssl;

#[cfg(feature = "openssl")]
pub mod openssl;

// Common code for BoringSSL/OpenSSL backends
pub use store::{create_tls_handler, get_global_tls_store};
pub use validation::{validate_cert, CertValidationError, CertValidationResult};
// Re-export gateway listener TLS matcher helpers from the runtime matching layer.
pub use crate::core::gateway::runtime::matching::{
    get_gateway_tls_matcher, rebuild_gateway_tls_matcher, GatewayTlsEntry, GatewayTlsMatcher,
};
