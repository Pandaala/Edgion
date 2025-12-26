//! mTLS client certificate verification at TLS handshake
//! 
//! This module uses unsafe FFI to set a custom SSL_set_verify callback
//! that validates SAN/CN whitelist during TLS handshake.

use crate::core::tls::cert_extractor::extract_client_cert_info;
use crate::core::tls::mtls_validator::{validate_cn_whitelist, validate_san_whitelist};
use crate::types::resources::edgion_tls::{ClientAuthConfig, EdgionTls};
use pingora_core::tls::ssl::SslRef;
use std::os::raw::{c_int, c_void};
use std::sync::Arc;

/// Context data passed to verify callback via SSL ex_data
struct VerifyContext {
    client_auth: ClientAuthConfig,
    edgion_tls: Arc<EdgionTls>,
}

/// Global ex_data index for verify context (initialized once)
static mut VERIFY_EX_DATA_IDX: c_int = -1;
static INIT_ONCE: std::sync::Once = std::sync::Once::new();

/// Initialize ex_data index (called once)
unsafe fn init_ex_data_idx() {
    INIT_ONCE.call_once(|| {
        let idx = boring_sys::SSL_get_ex_new_index(
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            None,
            Some(free_verify_context),
        );
        VERIFY_EX_DATA_IDX = idx;
        if idx >= 0 {
            tracing::debug!("Initialized mTLS verify ex_data index: {}", idx);
        } else {
            tracing::error!("Failed to initialize mTLS verify ex_data index");
        }
    });
}

/// Free callback context when SSL is destroyed
unsafe extern "C" fn free_verify_context(
    _parent: *mut c_void,
    ptr: *mut c_void,
    _ad: *mut boring_sys::CRYPTO_EX_DATA,
    _idx: c_int,
    _argl: std::os::raw::c_long,
    _argp: *mut c_void,
) {
    if !ptr.is_null() {
        let _ = Box::from_raw(ptr as *mut VerifyContext);
        tracing::trace!("Freed mTLS verify context");
    }
}

/// SSL_set_verify callback for SAN/CN whitelist validation
/// Called during TLS handshake after cert chain verification
/// Returns 1 for accept, 0 for reject
unsafe extern "C" fn verify_callback(
    _preverify_ok: c_int,
    x509_ctx: *mut boring_sys::X509_STORE_CTX,
) -> c_int {
    // Get SSL from X509_STORE_CTX
    let ssl_ptr = boring_sys::X509_STORE_CTX_get_ex_data(
        x509_ctx,
        boring_sys::SSL_get_ex_data_X509_STORE_CTX_idx(),
    ) as *mut boring_sys::SSL;
    
    if ssl_ptr.is_null() {
        tracing::error!("verify_callback: failed to get SSL from X509_STORE_CTX");
        return 0;
    }
    
    // Get our context from SSL ex_data
    let idx = VERIFY_EX_DATA_IDX;
    if idx < 0 {
        // No whitelist configured, allow
        return 1;
    }
    
    let ctx_ptr = boring_sys::SSL_get_ex_data(ssl_ptr, idx);
    if ctx_ptr.is_null() {
        // No context, allow (whitelist not configured)
        return 1;
    }
    
    let context = &*(ctx_ptr as *const VerifyContext);
    
    // Convert SSL* to &SslRef using std::mem::transmute
    // This is safe because SslRef is a transparent wrapper
    let ssl_ref: &SslRef = std::mem::transmute(&*ssl_ptr);
    
    // Perform whitelist validation
    if perform_whitelist_validation(ssl_ref, &context.client_auth, &context.edgion_tls) {
        1 // Accept
    } else {
        0 // Reject
    }
}

/// Perform SAN/CN whitelist validation (safe Rust code)
fn perform_whitelist_validation(
    ssl: &SslRef,
    client_auth: &ClientAuthConfig,
    edgion_tls: &Arc<EdgionTls>,
) -> bool {
    // Extract client certificate
    let cert_info = match extract_client_cert_info(ssl) {
        Some(info) => info,
        None => {
            tracing::warn!("No client certificate in verify callback");
            return false;
        }
    };
    
    tracing::debug!(
        subject = %cert_info.subject,
        sans = ?cert_info.sans,
        cn = ?cert_info.cn,
        "Validating client certificate whitelist"
    );
    
    // Validate SAN whitelist
    if let Some(ref allowed_sans) = client_auth.allowed_sans {
        if !validate_san_whitelist(&cert_info, allowed_sans) {
            tracing::warn!(
                subject = %cert_info.subject,
                sans = ?cert_info.sans,
                allowed = ?allowed_sans,
                "SAN whitelist validation failed"
            );
            return false;
        }
    }
    
    // Validate CN whitelist
    if let Some(ref allowed_cns) = client_auth.allowed_cns {
        if !validate_cn_whitelist(&cert_info, allowed_cns) {
            tracing::warn!(
                subject = %cert_info.subject,
                cn = ?cert_info.cn,
                allowed = ?allowed_cns,
                "CN whitelist validation failed"
            );
            return false;
        }
    }
    
    tracing::info!(
        subject = %cert_info.subject,
        fingerprint = %cert_info.fingerprint,
        "Client certificate whitelist validation passed"
    );
    
    true
}

/// Set custom verify callback with SAN/CN whitelist validation
///
/// This function uses unsafe FFI to set SSL_set_verify with a custom callback.
/// The callback is invoked during TLS handshake after certificate chain verification.
///
/// # Safety
/// Uses unsafe FFI but ensures proper memory management via ex_data mechanism.
pub fn set_verify_callback_with_whitelist(
    ssl: &mut SslRef,
    verify_mode: pingora_core::tls::ssl::SslVerifyMode,
    client_auth: &ClientAuthConfig,
    edgion_tls: &Arc<EdgionTls>,
) -> Result<(), String> {
    unsafe {
        // Initialize ex_data index
        init_ex_data_idx();
        
        let idx = VERIFY_EX_DATA_IDX;
        if idx < 0 {
            return Err("Failed to get ex_data index".to_string());
        }
        
        // Create context
        let context = Box::new(VerifyContext {
            client_auth: client_auth.clone(),
            edgion_tls: edgion_tls.clone(),
        });
        
        let ctx_ptr = Box::into_raw(context) as *mut c_void;
        
        // Get raw SSL pointer
        let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;
        
        // Store context in ex_data
        if boring_sys::SSL_set_ex_data(ssl_ptr, idx, ctx_ptr) == 0 {
            let _ = Box::from_raw(ctx_ptr as *mut VerifyContext);
            return Err("Failed to set ex_data".to_string());
        }
        
        // Set verify mode with custom callback
        boring_sys::SSL_set_verify(
            ssl_ptr,
            verify_mode.bits() as c_int,
            Some(verify_callback),
        );
        
        tracing::debug!("Set custom mTLS verify callback with SAN/CN whitelist");
        Ok(())
    }
}
