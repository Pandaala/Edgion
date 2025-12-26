pub mod cert_validator;
pub mod tls_cert_matcher;
pub mod tls_store;
pub mod mtls_validator;
mod conf_handler_impl;

// Backend-specific modules
#[cfg(feature = "boringssl")]
pub mod boringssl;

#[cfg(feature = "openssl")]
pub mod openssl;

// Common code for BoringSSL/OpenSSL backends
#[cfg(any(feature = "boringssl", feature = "openssl"))]
pub mod backend_common;

pub use cert_validator::{validate_cert, CertValidationResult, CertValidationError};

pub use conf_handler_impl::create_tls_handler;
pub use tls_store::get_global_tls_store;
