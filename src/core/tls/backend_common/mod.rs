#![cfg(any(feature = "boringssl", feature = "openssl"))]

pub mod backend_api;
pub mod cert_extractor;

// Keep legacy path for tls_pingora (now in gateway_common).
pub mod tls_pingora {
    pub use crate::core::tls::gateway_common::tls_pingora::*;
}

// Re-export for convenience
pub use backend_api::set_mtls_verify_callback;
