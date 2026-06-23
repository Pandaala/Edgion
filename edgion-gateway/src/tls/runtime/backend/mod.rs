#![cfg(feature = "boringssl")]

pub mod backend_api;
pub mod cert_extractor;

// Alias: re-exports tls_pingora from runtime/gateway for callers importing via backend::tls_pingora.
pub mod tls_pingora {
    pub use crate::tls::runtime::gateway::tls_pingora::*;
}

pub use backend_api::set_mtls_verify_callback;
