//! mTLS client certificate verification at TLS handshake
//! 
//! This module uses unsafe FFI to set a custom SSL_set_verify callback
//! that validates SAN/CN whitelist during TLS handshake.

use crate::core::tls::cert_extractor::extract_client_cert_info;
use crate::core::tls::mtls_validator::{validate_cn_whitelist, validate_san_whitelist};
use crate::types::resources::edgion_tls::{ClientAuthConfig, EdgionTls};
use pingora_core::tls::ssl::SslRef;
use std::os::raw::{c_int, c_void};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

/// Context data passed to verify callback via SSL ex_data
/// Only stores Arc<EdgionTls> - client_auth is accessed when needed
struct VerifyContext {
    edgion_tls: Arc<EdgionTls>,
}

/// Global ex_data index for verify context
/// Uses AtomicI32 for thread-safe access without unsafe static mut
static VERIFY_EX_DATA_IDX: AtomicI32 = AtomicI32::new(-1);
static INIT_ONCE: std::sync::Once = std::sync::Once::new();

/// Initialize ex_data index (called once)
/// Returns true if initialization succeeded, false if it failed
unsafe fn init_ex_data_idx() -> bool {
    INIT_ONCE.call_once(|| {
        let idx = boring_sys::SSL_get_ex_new_index(
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            None,
            Some(free_verify_context),
        );
        VERIFY_EX_DATA_IDX.store(idx, Ordering::Release);
        if idx >= 0 {
            tracing::debug!("Initialized mTLS verify ex_data index: {}", idx);
        } else {
            tracing::error!(
                "Failed to initialize mTLS verify ex_data index. \
                mTLS SAN/CN whitelist validation will not work!"
            );
        }
    });
    
    // Check if initialization succeeded
    VERIFY_EX_DATA_IDX.load(Ordering::Acquire) >= 0
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
///
/// # Safety
/// This is a C callback invoked by BoringSSL during TLS handshake.
/// We use `catch_unwind` to prevent panics from crossing the FFI boundary,
/// which would be undefined behavior.
///
/// # Panic Safety Limitations
/// `catch_unwind` can catch most panics, but NOT:
/// - Stack overflows (process will abort)
/// - Panics in `extern "C"` functions compiled with `panic=abort`
/// - Double panics (panic while unwinding from another panic)
/// - Process aborts via `std::process::abort()` or C `abort()`
///
/// These limitations are inherent to Rust's panic mechanism.
/// In production, ensure the verification logic is thoroughly tested
/// to avoid panic conditions.
unsafe extern "C" fn verify_callback(
    preverify_ok: c_int,
    x509_ctx: *mut boring_sys::X509_STORE_CTX,
) -> c_int {
    // CRITICAL: Catch all panics to prevent UB across FFI boundary
    // If Rust panics in a C callback, it's undefined behavior!
    let result = std::panic::catch_unwind(|| {
        verify_callback_impl(preverify_ok, x509_ctx)
    });
    
    match result {
        Ok(code) => code,
        Err(e) => {
            // Panic occurred - log and reject connection
            let panic_msg = if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            
            tracing::error!(
                panic = %panic_msg,
                "PANIC in TLS verify callback! Rejecting connection to prevent UB"
            );
            0 // Reject on panic
        }
    }
}

/// Internal implementation of verify callback (can safely panic)
unsafe fn verify_callback_impl(
    preverify_ok: c_int,
    x509_ctx: *mut boring_sys::X509_STORE_CTX,
) -> c_int {
    // First check if basic cert chain verification passed
    // If preverify_ok is 0, the certificate chain is invalid (expired, revoked, untrusted, etc.)
    // We MUST reject such certificates regardless of whitelist
    if preverify_ok == 0 {
        // Get error details for logging
        let error_code = boring_sys::X509_STORE_CTX_get_error(x509_ctx);
        
        // SAFETY: X509_verify_cert_error_string may return NULL for unknown error codes
        let error_ptr = boring_sys::X509_verify_cert_error_string(error_code as i64);
        let error_str = if error_ptr.is_null() {
            format!("Unknown error code: {}", error_code)
        } else {
            std::ffi::CStr::from_ptr(error_ptr).to_string_lossy().into_owned()
        };
        
        tracing::warn!(
            error_code = error_code,
            error = %error_str,
            "Certificate chain verification failed, rejecting connection"
        );
        return 0;
    }
    
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
    let idx = VERIFY_EX_DATA_IDX.load(Ordering::Acquire);
    if idx < 0 {
        // No whitelist configured, allow (preverify passed)
        return 1;
    }
    
    let ctx_ptr = boring_sys::SSL_get_ex_data(ssl_ptr, idx);
    if ctx_ptr.is_null() {
        // No context, allow (whitelist not configured)
        return 1;
    }
    
    let context = &*(ctx_ptr as *const VerifyContext);
    
    // Get client_auth from EdgionTls (no cloning needed!)
    let client_auth = match &context.edgion_tls.spec.client_auth {
        Some(auth) => auth,
        None => return 1, // No client auth configured, allow
    };
    
    // SAFETY: Convert SSL* to &SslRef
    // This transmute is safe because:
    // 1. SslRef is a transparent wrapper around boring_sys::SSL (foreign_types pattern)
    // 2. ssl_ptr is guaranteed valid during callback execution by BoringSSL
    // 3. We only create a reference, not taking ownership
    // 4. The reference lifetime is bounded by this function scope
    // 5. SslRef uses #[repr(transparent)] (implied by foreign_types)
    //
    // If SslRef's internal representation changes, this would need updating.
    // Alternative: Use Pingora's safe API if/when it exposes SSL in callbacks.
    let ssl_ref: &SslRef = std::mem::transmute(&*ssl_ptr);
    
    // Perform whitelist validation
    if perform_whitelist_validation(ssl_ref, client_auth) {
        1 // Accept
    } else {
        0 // Reject
    }
}

/// Perform SAN/CN whitelist validation (safe Rust code)
fn perform_whitelist_validation(
    ssl: &SslRef,
    client_auth: &ClientAuthConfig,
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
    
    // TODO(observability): Add metrics for:
    // - mtls_validation_total counter (with status label: passed/failed, reason label)
    // - mtls_san_whitelist_checks_total counter
    // - mtls_cn_whitelist_checks_total counter
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
/// # Important
/// This function should only be called ONCE per SSL object. Calling it multiple
/// times on the same SSL object is safe (BoringSSL will call the free callback
/// before replacing ex_data), but it's inefficient and may indicate a logic error.
///
/// # Performance
/// No cloning - client_auth is accessed directly from Arc<EdgionTls> when needed.
///
/// # Safety
/// Uses unsafe FFI but ensures proper memory management via ex_data mechanism.
/// BoringSSL guarantees that the free callback will be invoked when:
/// - The SSL object is destroyed
/// - ex_data is replaced by another SSL_set_ex_data call
pub fn set_verify_callback_with_whitelist(
    ssl: &mut SslRef,
    verify_mode: pingora_core::tls::ssl::SslVerifyMode,
    edgion_tls: &Arc<EdgionTls>,
) -> Result<(), String> {
    unsafe {
        // Initialize ex_data index and check if it succeeded
        if !init_ex_data_idx() {
            return Err(
                "Failed to initialize SSL ex_data index for mTLS verification. \
                This is a critical error - BoringSSL may have run out of ex_data slots. \
                mTLS SAN/CN whitelist validation cannot be enabled.".to_string()
            );
        }
        
        let idx = VERIFY_EX_DATA_IDX.load(Ordering::Acquire);
        
        // Get raw SSL pointer
        let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;
        
        // Check if ex_data already exists (indicates repeated call)
        let existing_ptr = boring_sys::SSL_get_ex_data(ssl_ptr, idx);
        if !existing_ptr.is_null() {
            tracing::warn!(
                "SSL ex_data already set for mTLS verification. \
                This indicates set_verify_callback_with_whitelist was called multiple times. \
                The old context will be freed automatically."
            );
            // BoringSSL will automatically call free_verify_context for the old data
        }
        
        // Create context - only Arc clone (cheap reference counting)
        let context = Box::new(VerifyContext {
            edgion_tls: edgion_tls.clone(),
        });
        
        let ctx_ptr = Box::into_raw(context) as *mut c_void;
        
        // Store context in ex_data
        // If this fails, we must manually free the context to avoid leak
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
