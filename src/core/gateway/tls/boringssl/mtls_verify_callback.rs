//! mTLS client certificate verification at TLS handshake
//!
//! This module uses unsafe FFI to set a custom SSL_set_verify callback
//! that validates SAN/CN whitelist during TLS handshake.
//!
//! **Note**: This module requires BoringSSL for custom verify callbacks.
//! It will only be compiled when the `boringssl` feature is enabled.

use crate::core::gateway::observe::logs::ssl_log::log_ssl;
use crate::core::gateway::tls::runtime::backend::cert_extractor::extract_client_cert_info;
use crate::core::gateway::tls::runtime::gateway::tls_pingora::peek_tls_conn_meta;
use crate::core::gateway::tls::validation::mtls::{validate_cn_whitelist, validate_san_whitelist};
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
unsafe extern "C" fn verify_callback(preverify_ok: c_int, x509_ctx: *mut boring_sys::X509_STORE_CTX) -> c_int {
    // CRITICAL: Catch all panics to prevent UB across FFI boundary
    // If Rust panics in a C callback, it's undefined behavior!
    let result = std::panic::catch_unwind(|| verify_callback_impl(preverify_ok, x509_ctx));

    result.unwrap_or_else(|e| {
        // Panic occurred - best-effort log to ssl.log and reject connection.
        // FFI calls here cannot panic in Rust sense; log_mtls_failure handles
        // null ssl_ptr and missing meta gracefully (silently drops the entry).
        let panic_msg = if let Some(s) = e.downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = e.downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_string()
        };

        let ssl_ptr = boring_sys::X509_STORE_CTX_get_ex_data(x509_ctx, boring_sys::SSL_get_ex_data_X509_STORE_CTX_idx())
            as *mut boring_sys::SSL;
        log_mtls_failure(ssl_ptr, &format!("verify callback panic: {}", panic_msg));
        0 // Reject on panic
    })
}

/// Internal implementation of verify callback (can safely panic)
unsafe fn verify_callback_impl(preverify_ok: c_int, x509_ctx: *mut boring_sys::X509_STORE_CTX) -> c_int {
    // First check if basic cert chain verification passed
    // If preverify_ok is 0, the certificate chain is invalid (expired, revoked, untrusted, etc.)
    // We MUST reject such certificates regardless of whitelist
    if preverify_ok == 0 {
        // Get error details for logging
        let error_code = boring_sys::X509_STORE_CTX_get_error(x509_ctx);

        // SAFETY: X509_verify_cert_error_string expects i64 (c_long on most platforms)
        // error_code is c_int, which is always safely convertible to i64
        let error_ptr = boring_sys::X509_verify_cert_error_string(error_code as i64);
        let error_str = if error_ptr.is_null() {
            format!("Unknown error code: {}", error_code)
        } else {
            std::ffi::CStr::from_ptr(error_ptr).to_string_lossy().into_owned()
        };

        // Try to log to ssl.log with connection context
        let ssl_ptr = boring_sys::X509_STORE_CTX_get_ex_data(x509_ctx, boring_sys::SSL_get_ex_data_X509_STORE_CTX_idx())
            as *mut boring_sys::SSL;
        log_mtls_failure(ssl_ptr, &format!("mTLS chain verify failed: {}", error_str));
        return 0;
    }

    // Get SSL from X509_STORE_CTX
    let ssl_ptr = boring_sys::X509_STORE_CTX_get_ex_data(x509_ctx, boring_sys::SSL_get_ex_data_X509_STORE_CTX_idx())
        as *mut boring_sys::SSL;

    if ssl_ptr.is_null() {
        // Defensive: BoringSSL guarantees a non-null SSL pointer in this callback
        // context. If null somehow, ssl.log can't be written (no conn meta), so
        // silently reject the connection.
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

    // SAFETY: ssl_ptr is valid during callback execution (guaranteed by BoringSSL).
    // SslRef is an opaque foreign_types wrapper over boring_sys::SSL;
    // this cast is the same as ForeignTypeRef::from_ptr().
    let ssl_ref: &SslRef = &*(ssl_ptr as *const SslRef);

    // Perform whitelist validation
    match perform_whitelist_validation(ssl_ref, client_auth) {
        Ok(()) => 1,
        Err(reason) => {
            log_mtls_failure(ssl_ptr, &reason);
            0
        }
    }
}

/// Perform SAN/CN whitelist validation (safe Rust code)
fn perform_whitelist_validation(ssl: &SslRef, client_auth: &ClientAuthConfig) -> Result<(), String> {
    // Extract client certificate
    let cert_info = match extract_client_cert_info(ssl) {
        Some(info) => info,
        None => return Err("mTLS no client certificate".to_string()),
    };

    // Validate SAN whitelist
    if let Some(ref allowed_sans) = client_auth.allowed_sans {
        if !validate_san_whitelist(&cert_info, allowed_sans) {
            return Err(format!(
                "mTLS SAN whitelist failed: subject={}, sans={:?}",
                cert_info.subject, cert_info.sans
            ));
        }
    }

    // Validate CN whitelist
    if let Some(ref allowed_cns) = client_auth.allowed_cns {
        if !validate_cn_whitelist(&cert_info, allowed_cns) {
            return Err(format!(
                "mTLS CN whitelist failed: subject={}, cn={:?}",
                cert_info.subject, cert_info.cn
            ));
        }
    }

    Ok(())
}

/// Log mTLS verification failure to ssl.log with connection context from SslCtx ex_data.
///
/// If ssl_ptr is null or SslCtx is unavailable, the failure is silently dropped — those paths
/// indicate the SSL lifecycle is already broken (store_ssl_ctx would have emitted ssl.log earlier).
unsafe fn log_mtls_failure(ssl_ptr: *mut boring_sys::SSL, reason: &str) {
    if ssl_ptr.is_null() {
        return;
    }
    if let Some(mut meta) = peek_tls_conn_meta(ssl_ptr) {
        meta.err_log = Some(reason.to_string());
        log_ssl(&meta);
    }
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
            return Err("Failed to initialize SSL ex_data index for mTLS verification. \
                This is a critical error - BoringSSL may have run out of ex_data slots. \
                mTLS SAN/CN whitelist validation cannot be enabled."
                .to_string());
        }

        let idx = VERIFY_EX_DATA_IDX.load(Ordering::Acquire);

        // Get raw SSL pointer
        let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;

        // Create context - only Arc clone (cheap reference counting)
        // If ex_data is already set (repeated call), BoringSSL will auto-invoke free_verify_context
        // for the old data before swapping in the new pointer.
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
        boring_sys::SSL_set_verify(ssl_ptr, verify_mode.bits() as c_int, Some(verify_callback));

        Ok(())
    }
}
