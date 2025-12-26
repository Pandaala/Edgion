//! Backend-agnostic API for TLS operations
//! 
//! This module provides unified interfaces that dispatch to backend-specific
//! implementations based on compile-time features. When adding a new TLS backend,
//! add the corresponding implementation branch to these functions.

use pingora_core::tls::ssl::{SslRef, SslVerifyMode};
use crate::types::resources::edgion_tls::EdgionTls;
use std::sync::Arc;

/// Set mTLS whitelist verification callback for SSL connection
/// 
/// This unified interface internally dispatches to the appropriate backend
/// implementation based on compile features. When adding a new backend,
/// simply add a new cfg branch here.
///
/// # Supported Backends
/// - BoringSSL: Full support via custom verify callback
/// - OpenSSL: Not yet implemented
///
/// # Errors
/// Returns error if the current backend doesn't support this feature
#[inline]
pub fn set_mtls_verify_callback(
    ssl: &mut SslRef,
    verify_mode: SslVerifyMode,
    edgion_tls: &Arc<EdgionTls>,
) -> Result<(), String> {
    #[cfg(feature = "boringssl")]
    {
        crate::core::tls::boringssl::mtls_verify_callback::set_verify_callback_with_whitelist(
            ssl, verify_mode, edgion_tls
        )
    }
    
    #[cfg(all(feature = "openssl", not(feature = "boringssl")))]
    {
        // OpenSSL custom verify callback not yet implemented
        // Future: Implement OpenSSL version if needed
        Err("mTLS SAN/CN whitelist validation not yet implemented for OpenSSL backend".to_string())
    }
}

// Future: Add more unified backend APIs here as needed
// Example:
// - pub fn configure_session_cache(...)
// - pub fn set_alpn_protocols(...)

