//! mTLS client certificate verification at TLS handshake.
//!
//! Sets a custom `SSL_set_verify` callback (BoringSSL FFI) that validates
//! the client cert chain against a configured SAN/CN whitelist.

use crate::observe::logs::ssl_log::log_ssl;
use crate::tls::runtime::backend::cert_extractor::{extract_client_cert_info, extract_presented_sans};
use crate::tls::runtime::gateway::tls_pingora::peek_tls_conn_meta;
use crate::tls::validation::mtls::{validate_cn_whitelist, validate_san_whitelist};
use edgion_common::startup::catch_redacted_panic;
use edgion_resources::resources::edgion_tls::{ClientAuthConfig, EdgionTls};
use pingora_core::tls::ssl::SslRef;
use std::os::raw::{c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;

/// Per-connection context passed to the verify callback via SSL ex_data.
struct VerifyContext {
    edgion_tls: Arc<EdgionTls>,
    /// One handshake owns at most one terminal mTLS `ssl.log` record.
    audit_emitted: AtomicBool,
}

static VERIFY_EX_DATA_IDX: AtomicI32 = AtomicI32::new(-1);
static INIT_ONCE: std::sync::Once = std::sync::Once::new();

/// BoringSSL invokes the verify callback at every chain depth. Only the leaf
/// may extract client identity or evaluate the SAN/CN allowlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerifyStage {
    RejectChain,
    AcceptIntermediate,
    RejectMissingContext,
    RejectInvalidDepth,
    EvaluateLeafAllowlist,
}

fn classify_verify_callback(preverify_ok: bool, depth: c_int, context_present: bool) -> VerifyStage {
    if !preverify_ok {
        VerifyStage::RejectChain
    } else if depth == 0 {
        if context_present {
            VerifyStage::EvaluateLeafAllowlist
        } else {
            VerifyStage::RejectMissingContext
        }
    } else if depth > 0 {
        VerifyStage::AcceptIntermediate
    } else {
        VerifyStage::RejectInvalidDepth
    }
}

fn claim_terminal_audit(flag: &AtomicBool) -> bool {
    flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

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
    if ptr.is_null() {
        return;
    }
    // A panic must never unwind across the BoringSSL callback boundary. The
    // context currently contains only an Arc, but keep the boundary safe if a
    // future field gains a fallible Drop implementation.
    let _ = catch_redacted_panic(|| unsafe {
        drop(Box::from_raw(ptr as *mut VerifyContext));
    });
}

/// SSL_set_verify callback for SAN/CN whitelist validation. Returns 1 to
/// accept, 0 to reject. Wraps the impl in the shared redacted panic boundary
/// because a Rust panic crossing into BoringSSL is UB and its hook must not
/// expose certificate-derived detail.
unsafe extern "C" fn verify_callback(preverify_ok: c_int, x509_ctx: *mut boring_sys::X509_STORE_CTX) -> c_int {
    let result = catch_redacted_panic(|| unsafe { verify_callback_impl(preverify_ok, x509_ctx) });

    result.unwrap_or_else(|_| {
        // Never expose a dynamic panic payload: it may contain certificate
        // identity or dependency detail. Best-effort log a static diagnostic
        // and reject the connection.
        if !x509_ctx.is_null() {
            let ssl_ptr =
                boring_sys::X509_STORE_CTX_get_ex_data(x509_ctx, boring_sys::SSL_get_ex_data_X509_STORE_CTX_idx())
                    as *mut boring_sys::SSL;
            emit_mtls_failure(ssl_ptr, "mTLS verify callback panic");
        }
        0 // Reject on panic
    })
}

unsafe fn ssl_from_store_ctx(x509_ctx: *mut boring_sys::X509_STORE_CTX) -> *mut boring_sys::SSL {
    boring_sys::X509_STORE_CTX_get_ex_data(x509_ctx, boring_sys::SSL_get_ex_data_X509_STORE_CTX_idx())
        as *mut boring_sys::SSL
}

unsafe fn verify_context_from_ssl(ssl_ptr: *mut boring_sys::SSL) -> Option<*const VerifyContext> {
    if ssl_ptr.is_null() {
        return None;
    }
    let idx = VERIFY_EX_DATA_IDX.load(Ordering::Acquire);
    if idx < 0 {
        return None;
    }
    let ctx_ptr = boring_sys::SSL_get_ex_data(ssl_ptr, idx);
    if ctx_ptr.is_null() {
        None
    } else {
        Some(ctx_ptr as *const VerifyContext)
    }
}

/// Emit a terminal mTLS audit.
///
/// When `VerifyContext` is present, `audit_emitted` claims the single handshake
/// record. When it is missing, this invocation is still the sole writer because
/// every caller returns 0 and BoringSSL stops the verify-callback walk.
unsafe fn emit_mtls_failure(ssl_ptr: *mut boring_sys::SSL, reason: &str) {
    if let Some(ctx_ptr) = verify_context_from_ssl(ssl_ptr) {
        let context = &*ctx_ptr;
        if !claim_terminal_audit(&context.audit_emitted) {
            return;
        }
    }
    log_mtls_failure(ssl_ptr, reason);
}

/// Verify callback impl. Safe to panic — wrapper above catches.
unsafe fn verify_callback_impl(preverify_ok: c_int, x509_ctx: *mut boring_sys::X509_STORE_CTX) -> c_int {
    if x509_ctx.is_null() {
        return 0;
    }

    let ssl_ptr = ssl_from_store_ctx(x509_ctx);
    let context_ptr = verify_context_from_ssl(ssl_ptr);
    let depth = boring_sys::X509_STORE_CTX_get_error_depth(x509_ctx);
    match classify_verify_callback(preverify_ok != 0, depth, context_ptr.is_some()) {
        VerifyStage::RejectChain => {
            // preverify_ok=0: the cert chain itself is invalid (expired, revoked,
            // untrusted, …). Reject at every depth. Returning 0 stops further
            // BoringSSL verify-callback invocations, so a missing VerifyContext
            // still yields one terminal audit for this handshake.
            let error_code = boring_sys::X509_STORE_CTX_get_error(x509_ctx);
            // SAFETY: X509_verify_cert_error_string takes c_long; c_int → i64 is lossless.
            let error_ptr = boring_sys::X509_verify_cert_error_string(error_code as i64);
            let error_str = if error_ptr.is_null() {
                format!("Unknown error code: {}", error_code)
            } else {
                std::ffi::CStr::from_ptr(error_ptr).to_string_lossy().into_owned()
            };
            emit_mtls_failure(ssl_ptr, &format!("mTLS chain verify failed: {}", error_str));
            0
        }
        VerifyStage::AcceptIntermediate => 1,
        VerifyStage::RejectInvalidDepth => {
            emit_mtls_failure(ssl_ptr, "mTLS verify depth invalid");
            0
        }
        VerifyStage::RejectMissingContext => {
            if ssl_ptr.is_null() {
                // Defensive: BoringSSL guarantees non-null here. Without an SSL we
                // can't write ssl.log, so silently reject.
                return 0;
            }
            let idx = VERIFY_EX_DATA_IDX.load(Ordering::Acquire);
            let reason = if idx < 0 {
                "mTLS verify context index missing"
            } else {
                "mTLS verify context missing"
            };
            emit_mtls_failure(ssl_ptr, reason);
            0
        }
        VerifyStage::EvaluateLeafAllowlist => {
            let Some(context_ptr) = context_ptr else {
                emit_mtls_failure(ssl_ptr, "mTLS verify context missing");
                return 0;
            };
            let context = &*context_ptr;
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
                    emit_mtls_failure(ssl_ptr, &reason);
                    0
                }
            }
        }
    }
}

fn perform_whitelist_validation(ssl: &SslRef, client_auth: &ClientAuthConfig) -> Result<(), String> {
    let cert_info = match extract_client_cert_info(ssl) {
        Some(info) => info,
        None => return Err("mTLS no client certificate".to_string()),
    };

    if let Some(ref allowed_sans) = client_auth.allowed_sans {
        let presented = ssl
            .peer_certificate()
            .map(|cert| extract_presented_sans(&cert))
            .unwrap_or_default();
        if !validate_san_whitelist(&presented, allowed_sans) {
            // Mirror the redaction ClientCertInfo gets via serde when normally
            // serialized — err_log embeds these as plain strings, so we have to
            // apply sensitive mode by hand here.
            let subject = edgion_resources::sensitive::apply_sensitive(&cert_info.subject, |s| s.to_string());
            let sans: Vec<String> = cert_info
                .sans
                .iter()
                .map(|s| edgion_resources::sensitive::apply_sensitive(s, |w| w.to_string()))
                .collect();
            return Err(format!(
                "mTLS SAN whitelist failed: subject={}, sans={:?}",
                subject, sans
            ));
        }
    }

    if let Some(ref allowed_cns) = client_auth.allowed_cns {
        if !validate_cn_whitelist(&cert_info, allowed_cns) {
            let subject = edgion_resources::sensitive::apply_sensitive(&cert_info.subject, |s| s.to_string());
            let cn = cert_info
                .cn
                .as_ref()
                .map(|s| edgion_resources::sensitive::apply_sensitive(s, |w| w.to_string()));
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

        let context = Box::new(VerifyContext {
            edgion_tls: edgion_tls.clone(),
            audit_emitted: AtomicBool::new(false),
        });
        let ctx_ptr = Box::into_raw(context) as *mut c_void;
        let prev = boring_sys::SSL_get_ex_data(ssl_ptr, idx);

        if boring_sys::SSL_set_ex_data(ssl_ptr, idx, ctx_ptr) == 0 {
            let _ = Box::from_raw(ctx_ptr as *mut VerifyContext);
            return Err("Failed to set ex_data".to_string());
        }
        // BoringSSL replaces the pointer without invoking the registered free
        // callback. Release the displaced value only after the new pointer is
        // installed, so a failed store cannot leave a dangling old slot.
        if !prev.is_null() {
            let _ = Box::from_raw(prev as *mut VerifyContext);
        }

        boring_sys::SSL_set_verify(ssl_ptr, verify_mode.bits() as c_int, Some(verify_callback));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    enum ChainOrder {
        LeafFirst,
        RootFirst,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ChainWalk {
        leaf_evaluations: usize,
        terminal_logs: usize,
        rejected: bool,
    }

    fn walk_verify_stages(
        cert_count: usize,
        order: ChainOrder,
        mut preverify_ok_at: impl FnMut(c_int) -> bool,
        context_present: bool,
        leaf_allowlist_ok: bool,
        stop_on_reject: bool,
    ) -> ChainWalk {
        assert!((1..=9).contains(&cert_count));
        let mut depths: Vec<c_int> = (0..cert_count as c_int).collect();
        if matches!(order, ChainOrder::RootFirst) {
            depths.reverse();
        }

        let audit = AtomicBool::new(false);
        let mut walk = ChainWalk {
            leaf_evaluations: 0,
            terminal_logs: 0,
            rejected: false,
        };

        for depth in depths {
            let stage = classify_verify_callback(preverify_ok_at(depth), depth, context_present);
            match stage {
                VerifyStage::RejectChain | VerifyStage::RejectMissingContext | VerifyStage::RejectInvalidDepth => {
                    walk.rejected = true;
                    if claim_terminal_audit(&audit) {
                        walk.terminal_logs += 1;
                    }
                    if stop_on_reject {
                        break;
                    }
                }
                VerifyStage::AcceptIntermediate => {}
                VerifyStage::EvaluateLeafAllowlist => {
                    walk.leaf_evaluations += 1;
                    if !leaf_allowlist_ok {
                        walk.rejected = true;
                        if claim_terminal_audit(&audit) {
                            walk.terminal_logs += 1;
                        }
                        if stop_on_reject {
                            break;
                        }
                    }
                }
            }
        }
        walk
    }

    #[test]
    fn classify_rejects_failed_preverify_at_every_depth() {
        for depth in 0..=8 {
            assert_eq!(classify_verify_callback(false, depth, true), VerifyStage::RejectChain);
            assert_eq!(classify_verify_callback(false, depth, false), VerifyStage::RejectChain);
        }
    }

    #[test]
    fn classify_accepts_intermediate_without_context() {
        for depth in 1..=8 {
            assert_eq!(
                classify_verify_callback(true, depth, false),
                VerifyStage::AcceptIntermediate
            );
            assert_eq!(
                classify_verify_callback(true, depth, true),
                VerifyStage::AcceptIntermediate
            );
        }
    }

    #[test]
    fn classify_leaf_requires_context_and_evaluates_allowlist() {
        assert_eq!(
            classify_verify_callback(true, 0, true),
            VerifyStage::EvaluateLeafAllowlist
        );
        assert_eq!(
            classify_verify_callback(true, 0, false),
            VerifyStage::RejectMissingContext
        );
    }

    #[test]
    fn classify_negative_depth_is_fail_closed() {
        assert_eq!(
            classify_verify_callback(true, -1, true),
            VerifyStage::RejectInvalidDepth
        );
    }

    #[test]
    fn successful_chains_evaluate_leaf_once_for_one_through_nine_certs() {
        for cert_count in 1..=9 {
            for order in [ChainOrder::LeafFirst, ChainOrder::RootFirst] {
                let walk = walk_verify_stages(cert_count, order, |_| true, true, true, true);
                assert_eq!(
                    walk,
                    ChainWalk {
                        leaf_evaluations: 1,
                        terminal_logs: 0,
                        rejected: false,
                    },
                    "cert_count={cert_count}"
                );
            }
        }
    }

    #[test]
    fn preverify_failure_rejects_immediately_without_leaf_work() {
        for cert_count in 1..=9 {
            for fail_depth in 0..cert_count as c_int {
                let walk = walk_verify_stages(
                    cert_count,
                    ChainOrder::RootFirst,
                    |depth| depth != fail_depth,
                    true,
                    true,
                    true,
                );
                assert!(walk.rejected, "fail_depth={fail_depth}");
                assert_eq!(walk.terminal_logs, 1, "fail_depth={fail_depth}");
                assert_eq!(walk.leaf_evaluations, 0, "fail_depth={fail_depth}");
            }
        }
    }

    #[test]
    fn missing_leaf_context_is_fail_closed() {
        let walk = walk_verify_stages(3, ChainOrder::LeafFirst, |_| true, false, true, true);
        assert_eq!(
            walk,
            ChainWalk {
                leaf_evaluations: 0,
                terminal_logs: 1,
                rejected: true,
            }
        );
    }

    #[test]
    fn intermediate_success_does_not_extract_or_log() {
        let walk = walk_verify_stages(4, ChainOrder::RootFirst, |_| true, true, true, true);
        assert_eq!(walk.leaf_evaluations, 1);
        assert_eq!(walk.terminal_logs, 0);
        assert!(!walk.rejected);
    }

    #[test]
    fn allowlist_failure_emits_one_terminal_log() {
        for order in [ChainOrder::LeafFirst, ChainOrder::RootFirst] {
            let walk = walk_verify_stages(5, order, |_| true, true, false, false);
            assert_eq!(
                walk,
                ChainWalk {
                    leaf_evaluations: 1,
                    terminal_logs: 1,
                    rejected: true,
                }
            );
        }
    }

    #[test]
    fn terminal_audit_is_claimed_once_across_repeated_failures() {
        let flag = AtomicBool::new(false);
        assert!(claim_terminal_audit(&flag));
        assert!(!claim_terminal_audit(&flag));
        assert!(!claim_terminal_audit(&flag));
    }
}
