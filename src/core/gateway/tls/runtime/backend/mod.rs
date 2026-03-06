#![cfg(any(feature = "boringssl", feature = "openssl"))]

pub mod backend_api;
pub mod cert_extractor;

// Keep a facade for code that still imports backend::tls_pingora while the
// canonical implementation now lives under runtime/gateway.
pub mod tls_pingora {
    pub use crate::core::gateway::tls::runtime::gateway::tls_pingora::*;
}

// Re-export for convenience
pub use backend_api::set_mtls_verify_callback;
