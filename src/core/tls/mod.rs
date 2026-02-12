pub mod cert_validator;
mod conf_handler_impl;
pub mod mtls_validator;
pub mod tls_cert_matcher;
pub mod tls_store;

// Backend-specific modules
#[cfg(feature = "boringssl")]
pub mod boringssl;

#[cfg(feature = "openssl")]
pub mod openssl;

// Common code for BoringSSL/OpenSSL backends
#[cfg(any(feature = "boringssl", feature = "openssl"))]
pub mod backend_common;

// Gateway (downstream) TLS callbacks and helpers
#[cfg(any(feature = "boringssl", feature = "openssl"))]
pub mod gateway_common;

pub use cert_validator::{validate_cert, CertValidationError, CertValidationResult};

pub use conf_handler_impl::create_tls_handler;
// Re-export from gateway module for backward compatibility
pub use crate::core::gateway::gateway::{
    get_gateway_tls_matcher, match_gateway_tls, rebuild_gateway_tls_matcher, GatewayTlsEntry, GatewayTlsMatcher,
};
pub use tls_store::get_global_tls_store;
