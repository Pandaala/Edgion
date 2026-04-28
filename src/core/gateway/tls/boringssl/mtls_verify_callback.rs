//! mTLS client certificate verification at TLS handshake.
//!
//! Sets a custom `SSL_set_verify` callback (BoringSSL FFI) that validates
//! the client cert chain against a configured SAN/CN whitelist.

use crate::core::gateway::observe::logs::ssl_log::log_ssl;
use crate::core::gateway::tls::runtime::backend::cert_extractor::extract_client_cert_info;
use crate::core::gateway::tls::runtime::gateway::tls_pingora::peek_tls_conn_meta;
use crate::core::gateway::tls::validation::mtls::{validate_cn_whitelist, validate_san_whitelist};
use crate::types::resources::edgion_tls::{ClientAuthConfig, EdgionTls};
use pingora_core::tls::ssl::SslRef;
use std::os::raw::{c_int, c_void};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

/// Per-connection context passed to the verify callback via SSL ex_data.
struct VerifyContext {
    edgion_tls: Arc<EdgionTls>,
}

static VERIFY_EX_DATA_IDX: AtomicI32 = AtomicI32::new(-1);
static INIT_ONCE: std::sync::Once = std::sync::Once::new();

/// Initialize the global ex_data index. Returns true if it's usable.
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

/// SSL_set_verify callback for SAN/CN whitelist validation. Returns 1 to
/// accept, 0 to reject. Wraps the impl in `catch_unwind` because a Rust
/// panic crossing the FFI boundary into BoringSSL is UB.
unsafe extern "C" fn verify_callback(preverify_ok: c_int, x509_ctx: *mut boring_sys::X509_STORE_CTX) -> c_int {
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

/// Verify callback impl. Safe to panic — wrapper above catches.
unsafe fn verify_callback_impl(preverify_ok: c_int, x509_ctx: *mut boring_sys::X509_STORE_CTX) -> c_int {
    // preverify_ok=0 means the cert chain itself is invalid (expired, revoked,
    // untrusted, …). Reject regardless of whitelist.
    if preverify_ok == 0 {
        let error_code = boring_sys::X509_STORE_CTX_get_error(x509_ctx);
        // SAFETY: X509_verify_cert_error_string takes c_long; c_int → i64 is lossless.
        let error_ptr = boring_sys::X509_verify_cert_error_string(error_code as i64);
        let error_str = if error_ptr.is_null() {
            format!("Unknown error code: {}", error_code)
        } else {
            std::ffi::CStr::from_ptr(error_ptr).to_string_lossy().into_owned()
        };

        let ssl_ptr = boring_sys::X509_STORE_CTX_get_ex_data(x509_ctx, boring_sys::SSL_get_ex_data_X509_STORE_CTX_idx())
            as *mut boring_sys::SSL;
        log_mtls_failure(ssl_ptr, &format!("mTLS chain verify failed: {}", error_str));
        return 0;
    }

    let ssl_ptr = boring_sys::X509_STORE_CTX_get_ex_data(x509_ctx, boring_sys::SSL_get_ex_data_X509_STORE_CTX_idx())
        as *mut boring_sys::SSL;
    if ssl_ptr.is_null() {
        // Defensive: BoringSSL guarantees non-null here. Without an SSL we
        // can't write ssl.log, so silently reject.
        return 0;
    }

    let idx = VERIFY_EX_DATA_IDX.load(Ordering::Acquire);
    if idx < 0 {
        return 1;
    }
    let ctx_ptr = boring_sys::SSL_get_ex_data(ssl_ptr, idx);
    if ctx_ptr.is_null() {
        // No whitelist context attached → preverify-passed cert is allowed.
        return 1;
    }

    let context = &*(ctx_ptr as *const VerifyContext);
    let client_auth = match &context.edgion_tls.spec.client_auth {
        Some(auth) => auth,
        None => return 1,
    };

    // SAFETY: ssl_ptr valid during callback (BoringSSL guarantee). SslRef is an
    // opaque wrapper over boring_sys::SSL — same as ForeignTypeRef::from_ptr().
    let ssl_ref: &SslRef = &*(ssl_ptr as *const SslRef);

    match perform_whitelist_validation(ssl_ref, client_auth) {
        Ok(()) => 1,
        Err(reason) => {
            log_mtls_failure(ssl_ptr, &reason);
            0
        }
    }
}

fn perform_whitelist_validation(ssl: &SslRef, client_auth: &ClientAuthConfig) -> Result<(), String> {
    let cert_info = match extract_client_cert_info(ssl) {
        Some(info) => info,
        None => return Err("mTLS no client certificate".to_string()),
    };

    if let Some(ref allowed_sans) = client_auth.allowed_sans {
        if !validate_san_whitelist(&cert_info, allowed_sans) {
            // Mirror the redaction ClientCertInfo gets via serde when normally
            // serialized — err_log embeds these as plain strings, so we have to
            // apply sensitive mode by hand here.
            let subject = crate::types::sensitive::apply_sensitive(&cert_info.subject, |s| s.to_string());
            let sans: Vec<String> = cert_info
                .sans
                .iter()
                .map(|s| crate::types::sensitive::apply_sensitive(s, |w| w.to_string()))
                .collect();
            return Err(format!(
                "mTLS SAN whitelist failed: subject={}, sans={:?}",
                subject, sans
            ));
        }
    }

    if let Some(ref allowed_cns) = client_auth.allowed_cns {
        if !validate_cn_whitelist(&cert_info, allowed_cns) {
            let subject = crate::types::sensitive::apply_sensitive(&cert_info.subject, |s| s.to_string());
            let cn = cert_info
                .cn
                .as_ref()
                .map(|s| crate::types::sensitive::apply_sensitive(s, |w| w.to_string()));
            return Err(format!("mTLS CN whitelist failed: subject={}, cn={:?}", subject, cn));
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

/// Install the SAN/CN whitelist verify callback on `ssl`.
///
/// Expected to be called at most once per SSL object (during cert_cb in
/// configure_mtls). Defensive on repeated calls: reclaims any prior
/// `Box<VerifyContext>` from the slot before overwriting, since BoringSSL
/// only invokes the registered free callback on SSL destruction (not on
/// `SSL_set_ex_data` overwrite). Mirrors `store_ssl_ctx` in tls_pingora.rs.
pub fn set_verify_callback_with_whitelist(
    ssl: &mut SslRef,
    verify_mode: pingora_core::tls::ssl::SslVerifyMode,
    edgion_tls: &Arc<EdgionTls>,
) -> Result<(), String> {
    unsafe {
        if !init_ex_data_idx() {
            return Err("Failed to initialize SSL ex_data index for mTLS verification. \
                This is a critical error - BoringSSL may have run out of ex_data slots. \
                mTLS SAN/CN whitelist validation cannot be enabled."
                .to_string());
        }

        let idx = VERIFY_EX_DATA_IDX.load(Ordering::Acquire);
        let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;

        let prev = boring_sys::SSL_get_ex_data(ssl_ptr, idx);
        if !prev.is_null() {
            let _ = Box::from_raw(prev as *mut VerifyContext);
        }

        let context = Box::new(VerifyContext {
            edgion_tls: edgion_tls.clone(),
        });
        let ctx_ptr = Box::into_raw(context) as *mut c_void;

        if boring_sys::SSL_set_ex_data(ssl_ptr, idx, ctx_ptr) == 0 {
            let _ = Box::from_raw(ctx_ptr as *mut VerifyContext);
            return Err("Failed to set ex_data".to_string());
        }

        boring_sys::SSL_set_verify(ssl_ptr, verify_mode.bits() as c_int, Some(verify_callback));
        Ok(())
    }
}
