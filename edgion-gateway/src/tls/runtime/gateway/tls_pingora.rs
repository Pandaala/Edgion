use crate::observe::logs::ssl_log::log_ssl;
use crate::observe::logs::LogBuffer;
use crate::runtime::matching::tls::CompiledServingCertificate;
use crate::runtime::matching::{FrontendMtlsOutcome, GatewayTlsEntry};
use crate::tls::runtime::backend::cert_extractor::extract_client_cert_info;
use crate::tls::runtime::backend::set_mtls_verify_callback;
use crate::tls::store::cert_matcher::get_tls_cert_matcher;
use crate::tls::store::compiled_tls::CompiledEdgionTls;
use crate::{MatchedInfo, Rsc, TlsConnMeta};
use anyhow::anyhow;
use anyhow::Result;
use edgion_resources::listener_key::ListenerKey;
use edgion_resources::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode, EdgionTls};
use edgion_resources::ResourceMeta;
#[cfg(feature = "boringssl")]
use foreign_types::{ForeignType, ForeignTypeRef};
use pingora_core::listeners::tls::TlsSettings;
use pingora_core::listeners::TlsAccept;
use pingora_core::protocols::tls::TlsRef;
use pingora_core::tls::ssl::{ClientHello, NameType, SelectCertError, SslRef, SslVerifyMode};
use pingora_core::{Error as PingoraError, ErrorType};
use rand::random;
use std::ops::DerefMut;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// SslCtx: per-connection TLS context passed between callbacks via ex_data
// ---------------------------------------------------------------------------

/// Per-connection TLS context. Created in select_cb, consumed by cert_cb
/// and handshake_complete_cb via SSL ex_data, finally moved into Pingora's
/// digest extension.
#[derive(Clone)]
struct SslCtx {
    failure_logged: bool,
    meta: TlsConnMeta,
    matched_edgion_tls: Option<Arc<CompiledEdgionTls>>,
    matched_gateway_tls: Option<Arc<GatewayTlsEntry>>,
    gateway_edgion_tls_overlay: Option<Arc<CompiledEdgionTls>>,
}

#[cfg(feature = "boringssl")]
fn install_compiled_serving_identity(ssl: &mut SslRef, certificate: &CompiledServingCertificate) -> bool {
    unsafe {
        // BoringSSL's implementation clears the legacy credential before it
        // begins setting the new chain. Mirror that contract explicitly so a
        // replacement can never fall back to an earlier identity.
        boring_sys::SSL_certs_clear(ssl.as_ptr());
        let installed = boring_sys::SSL_set_chain_and_key(
            ssl.as_ptr(),
            certificate.atomic_chain.as_ptr(),
            certificate.atomic_chain.len(),
            certificate.private_key.as_ptr(),
            std::ptr::null(),
        ) == 1;
        if !installed {
            boring_sys::SSL_certs_clear(ssl.as_ptr());
            // Consume the thread-local queue here. Leaving it populated would
            // attribute this cold identity failure to an unrelated TLS call.
            let _ = boring::error::ErrorStack::get();
        }
        installed
    }
}

impl SslCtx {
    fn new(port: u16) -> Self {
        Self {
            failure_logged: false,
            meta: TlsConnMeta {
                ts: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64,
                start_at: Instant::now(),
                handshake_complete_time: None,
                port: Some(port),
                tls_id: None,
                sni: None,
                log: None,
                matched: None,
                err_log: None,
                client_cert_info: None,
                is_mtls: false,
            },
            matched_edgion_tls: None,
            matched_gateway_tls: None,
            gateway_edgion_tls_overlay: None,
        }
    }
}

// ---------------------------------------------------------------------------
// BoringSSL ex_data helpers
// ---------------------------------------------------------------------------

#[cfg(feature = "boringssl")]
mod ssl_ctx_ex_data {
    use super::SslCtx;
    use crate::observe::logs::LogBuffer;
    use edgion_common::startup::catch_redacted_panic;
    use pingora_core::protocols::tls::TlsRef;
    use pingora_core::tls::ssl::SslRef;
    use std::os::raw::{c_int, c_long, c_void};
    use std::sync::atomic::{AtomicI32, Ordering};

    static SSL_CTX_EX_DATA_IDX: AtomicI32 = AtomicI32::new(-1);
    static SSL_CTX_INIT_ONCE: std::sync::Once = std::sync::Once::new();

    unsafe fn init_ssl_ctx_ex_data_idx() -> bool {
        SSL_CTX_INIT_ONCE.call_once(|| {
            let idx = boring_sys::SSL_get_ex_new_index(
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                None,
                Some(free_ssl_ctx),
            );
            SSL_CTX_EX_DATA_IDX.store(idx, Ordering::Release);
            if idx >= 0 {
                tracing::debug!("Initialized SslCtx ex_data index: {}", idx);
            } else {
                tracing::error!("Failed to initialize SslCtx ex_data index");
            }
        });
        SSL_CTX_EX_DATA_IDX.load(Ordering::Acquire) >= 0
    }

    /// BoringSSL-invoked free callback for SSL ex_data. Reclaims the Box and,
    /// emits any unclaimed failure audit, before or after certificate installation.
    /// Successful completion consumes the slot; mTLS rejection claims its audit
    /// before logging, so neither produces a duplicate during destruction.
    unsafe extern "C" fn free_ssl_ctx(
        _parent: *mut c_void,
        ptr: *mut c_void,
        _ad: *mut boring_sys::CRYPTO_EX_DATA,
        _idx: c_int,
        _argl: c_long,
        _argp: *mut c_void,
    ) {
        if ptr.is_null() {
            return;
        }
        // Keep both the panic hook and a potentially panicking payload
        // destructor inside the shared redacted FFI boundary.
        let _ = catch_redacted_panic(|| unsafe {
            let mut ssl_ctx = Box::from_raw(ptr as *mut SslCtx);
            if !ssl_ctx.failure_logged {
                // Preserve a more specific upstream err_log (e.g. "min ver: …")
                // if select_cb already set one.
                if ssl_ctx.meta.err_log.is_none() {
                    ssl_ctx.meta.err_log = Some(
                        if ssl_ctx.meta.tls_id.is_some() {
                            "handshake failed"
                        } else {
                            "early abort"
                        }
                        .to_string(),
                    );
                }
                crate::observe::logs::ssl_log::log_ssl(&ssl_ctx.meta);
            }
        });
    }

    /// Store SslCtx into SSL object via ex_data. Returns false and emits ssl.log on failure.
    pub(super) fn store_ssl_ctx(ssl: &mut SslRef, ctx: &SslCtx) -> bool {
        unsafe {
            if !init_ssl_ctx_ex_data_idx() {
                let mut log_meta = ctx.meta.clone();
                let _ = log_meta
                    .log
                    .get_or_insert_with(LogBuffer::new)
                    .push("exdata init failed");
                crate::observe::logs::ssl_log::log_ssl(&log_meta);
                return false;
            }
            let idx = SSL_CTX_EX_DATA_IDX.load(Ordering::Acquire);
            let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;

            let ptr = Box::into_raw(Box::new(ctx.clone())) as *mut c_void;
            let prev = boring_sys::SSL_get_ex_data(ssl_ptr, idx);
            if boring_sys::SSL_set_ex_data(ssl_ptr, idx, ptr) == 0 {
                let _ = Box::from_raw(ptr as *mut SslCtx);
                let mut log_meta = ctx.meta.clone();
                let _ = log_meta
                    .log
                    .get_or_insert_with(LogBuffer::new)
                    .push("exdata store failed");
                crate::observe::logs::ssl_log::log_ssl(&log_meta);
                return false;
            }
            // BoringSSL overwrites without invoking the free callback. Release
            // the displaced value only after the new pointer is installed, so
            // a failed store leaves the old slot and allocation intact.
            if !prev.is_null() {
                let _ = Box::from_raw(prev as *mut SslCtx);
            }
            true
        }
    }

    /// Claim the pending failure audit without taking ex-data ownership.
    /// Used by mTLS verify callback to write ssl.log on failure.
    ///
    /// # Safety
    /// `ssl_ptr` must be valid with exclusive, serialized callback access during the call.
    pub(super) unsafe fn claim_tls_failure_meta(ssl_ptr: *const boring_sys::SSL) -> Option<crate::ctx::TlsConnMeta> {
        let idx = SSL_CTX_EX_DATA_IDX.load(Ordering::Acquire);
        if idx < 0 {
            return None;
        }
        let ptr = boring_sys::SSL_get_ex_data(ssl_ptr, idx);
        if ptr.is_null() {
            return None;
        }
        let ctx = &mut *(ptr as *mut SslCtx);
        if ctx.failure_logged {
            return None;
        }
        let meta = ctx.meta.clone();
        ctx.failure_logged = true;
        Some(meta)
    }

    /// Take SslCtx from SSL object. Transfers ownership back via Box::from_raw
    /// and clears the ex_data slot to prevent double-free in the free callback.
    pub(super) fn take_ssl_ctx(ssl: &TlsRef) -> Option<SslCtx> {
        unsafe {
            let idx = SSL_CTX_EX_DATA_IDX.load(Ordering::Acquire);
            if idx < 0 {
                return None;
            }
            let ssl_ptr = ssl as *const TlsRef as *const boring_sys::SSL;
            let ptr = boring_sys::SSL_get_ex_data(ssl_ptr, idx);
            if ptr.is_null() {
                return None;
            }
            // Clear the slot before reconstructing ownership. If the setter
            // ever fails, BoringSSL must retain the live allocation for its
            // registered destructor instead of retaining a dangling pointer.
            if boring_sys::SSL_set_ex_data(ssl_ptr as *mut boring_sys::SSL, idx, std::ptr::null_mut()) == 0 {
                return None;
            }
            let ctx = Box::from_raw(ptr as *mut SslCtx);
            Some(*ctx)
        }
    }
}

#[cfg(not(feature = "boringssl"))]
mod ssl_ctx_ex_data {
    use super::SslCtx;
    use crate::observe::logs::LogBuffer;
    use pingora_core::protocols::tls::TlsRef;
    use pingora_core::tls::ssl::SslRef;

    pub(super) fn store_ssl_ctx(_ssl: &mut SslRef, ctx: &SslCtx) -> bool {
        let mut log_meta = ctx.meta.clone();
        let _ = log_meta
            .log
            .get_or_insert_with(LogBuffer::new)
            .push("no boringssl exdata");
        crate::observe::logs::ssl_log::log_ssl(&log_meta);
        false
    }

    pub(super) fn take_ssl_ctx(_ssl: &TlsRef) -> Option<SslCtx> {
        None
    }
}

use ssl_ctx_ex_data::{store_ssl_ctx, take_ssl_ctx};

/// Claim the pending failure audit and clone its metadata without taking ex-data ownership.
/// Used by mTLS verify callback to write ssl.log on handshake failure.
///
/// # Safety
/// `ssl_ptr` must be valid with exclusive, serialized callback access during the call.
#[cfg(feature = "boringssl")]
pub(crate) unsafe fn claim_tls_failure_meta(ssl_ptr: *const boring_sys::SSL) -> Option<crate::ctx::TlsConnMeta> {
    ssl_ctx_ex_data::claim_tls_failure_meta(ssl_ptr)
}

// ---------------------------------------------------------------------------
// TlsCallback
// ---------------------------------------------------------------------------

/// TLS callback handler. All SNI resolution and matching state lives in the
/// `select_certificate_callback` closure (captured at builder time); this
/// struct only retains `port` for fallback TlsConnMeta logging.
pub struct TlsCallback {
    port: u16,
}

#[async_trait::async_trait]
impl TlsAccept for TlsCallback {
    async fn certificate_callback(&self, ssl: &mut TlsRef) {
        // select_cb is the single point of SNI resolution + Layer 1/2 matching
        // and fail-closes on store_ssl_ctx failure. So if cert_cb
        // runs at all, the snapshot must exist with a recorded match.
        //
        // take_ssl_ctx returning None here means BoringSSL violated its state
        // machine (e.g. invoked cert_cb without select_cb, or zeroed ex_data
        // between the two callbacks). Fail loud rather than silently re-run
        // matching — this branch is theoretically unreachable.
        let Some(mut ssl_ctx) = take_ssl_ctx(ssl) else {
            let mut ssl_ctx = SslCtx::new(self.port);
            ssl_ctx.meta.err_log = Some("no snapshot".to_string());
            log_ssl(&ssl_ctx.meta);
            return;
        };

        // Each apply_*_cert only sets err_log on failure; on failure we log_ssl
        // here and skip finalize. Without finalize, ex_data stays null (cert_cb
        // entry's take_ssl_ctx cleared it), so free_ssl_ctx will see a null ptr
        // and not double-emit. On success we finalize and let
        // handshake_complete_callback emit the single ssl.log entry.
        if let (Some(gateway_tls), Some(edgion_tls)) = (
            ssl_ctx.matched_gateway_tls.as_ref(),
            ssl_ctx.gateway_edgion_tls_overlay.as_ref(),
        ) {
            match self.configure_gateway_frontend_mtls(ssl, gateway_tls) {
                Err(e) => {
                    ssl_ctx.meta.err_log = Some(format!("frontend mTLS error: {e}"));
                    log_ssl(&ssl_ctx.meta);
                    return;
                }
                Ok(verify_installed) => ssl_ctx.meta.is_mtls = verify_installed,
            }
            let edgion_tls = Arc::clone(edgion_tls);
            self.apply_edgion_tls_cert(ssl, &edgion_tls, &mut ssl_ctx);
            if ssl_ctx.meta.err_log.is_some() {
                log_ssl(&ssl_ctx.meta);
                return;
            }
            self.finalize_ssl_ctx(ssl, &mut ssl_ctx);
            if ssl_ctx.meta.err_log.is_some() {
                log_ssl(&ssl_ctx.meta);
            }
            return;
        }
        if let Some(edgion_tls) = ssl_ctx.matched_edgion_tls.clone() {
            self.apply_edgion_tls_cert(ssl, &edgion_tls, &mut ssl_ctx);
            if ssl_ctx.meta.err_log.is_some() {
                log_ssl(&ssl_ctx.meta);
                return;
            }
            self.finalize_ssl_ctx(ssl, &mut ssl_ctx);
            if ssl_ctx.meta.err_log.is_some() {
                log_ssl(&ssl_ctx.meta);
            }
            return;
        }
        if let Some(gateway_tls) = ssl_ctx.matched_gateway_tls.clone() {
            self.apply_gateway_tls_cert(ssl, &gateway_tls, &mut ssl_ctx);
            if ssl_ctx.meta.err_log.is_some() {
                log_ssl(&ssl_ctx.meta);
                return;
            }
            self.finalize_ssl_ctx(ssl, &mut ssl_ctx);
            if ssl_ctx.meta.err_log.is_some() {
                log_ssl(&ssl_ctx.meta);
            }
            return;
        }

        // Snapshot present but neither match recorded — impossible state given
        // select_cb's contract. Fail loud.
        ssl_ctx.meta.err_log = Some("snapshot empty".to_string());
        log_ssl(&ssl_ctx.meta);
    }

    async fn handshake_complete_callback(&self, ssl: &TlsRef) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
        if let Some(mut ssl_ctx) = take_ssl_ctx(ssl) {
            // Extract client cert info now that handshake is complete (mTLS verified)
            if let Some(ref edgion_tls) = ssl_ctx.matched_edgion_tls {
                if edgion_tls.tls.is_mtls_enabled() && edgion_tls.tls.should_expose_client_cert() {
                    ssl_ctx.meta.client_cert_info = extract_client_cert_info(ssl);
                }
            } else if ssl_ctx.meta.is_mtls {
                // Gateway frontendValidation mTLS: always extract client cert info
                ssl_ctx.meta.client_cert_info = extract_client_cert_info(ssl);
            }

            ssl_ctx.meta.handshake_complete_time = Some(
                Instant::now()
                    .saturating_duration_since(ssl_ctx.meta.start_at)
                    .as_millis() as u64,
            );
            log_ssl(&ssl_ctx.meta);

            return Some(Arc::new(ssl_ctx.meta));
        }

        // Fallback when no SslCtx is recoverable from ex_data. Returns a minimal
        // TlsConnMeta so downstream consumers (TLS proxy, HTTP proxy) can still
        // read it from the digest.
        //
        // Intentionally does NOT call log_ssl here — preserving the
        // "one ssl.log per connection" audit invariant (tls-F004):
        // * BoringSSL builds: successful select_cb has already created and
        //   sized the ex_data stack. take_ssl_ctx clears the existing slot,
        //   and pinned BoringSSL 5.2 replaces that same slot in finalize
        //   without allocating, so normal completion recovers the context.
        //   Index-init or first-store failures log and abort in select_cb
        //   before this callback can run.
        // * Non-BoringSSL builds: store_ssl_ctx in select_cb always fails and
        //   returns SelectCertError::ERROR, so cert_cb / this callback are not
        //   reached on that path.
        // Reaching this fallback therefore signals an unknown state-machine or
        // ex_data contract violation. It cannot determine whether an unknown
        // path already emitted, so it returns minimal metadata without
        // synthesizing a potentially duplicate audit entry.
        let meta = TlsConnMeta {
            ts: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
            start_at: Instant::now(),
            handshake_complete_time: None,
            port: Some(self.port),
            tls_id: None,
            sni: ssl.servername(NameType::HOST_NAME).map(|s| s.to_string()),
            log: Some({
                let mut buf = LogBuffer::new();
                let _ = buf.push("no ssl ctx");
                buf
            }),
            matched: None,
            err_log: None,
            client_cert_info: None,
            is_mtls: false,
        };
        Some(Arc::new(meta))
    }
}

/// Apply the selected identity's overrides against the single captured EGC.
/// Shared by every selection branch; no later callback may reload policy.
fn apply_selected_downstream_policy(
    ssl: &mut SslRef,
    selected: &SslCtx,
    policy: &crate::tls::downstream_policy::DownstreamTlsPolicy,
    enforce_fips: bool,
) -> std::result::Result<(), &'static str> {
    let local = selected
        .gateway_edgion_tls_overlay
        .as_ref()
        .or(selected.matched_edgion_tls.as_ref());
    policy.apply(
        ssl,
        local.and_then(|entry| entry.tls.spec.min_tls_version),
        local.map(|entry| &entry.operator_cipher),
        enforce_fips,
    )
}

fn edgion_tls_attaches_to_gateway(candidate: &EdgionTls, gateway: &GatewayTlsEntry) -> bool {
    if !gateway.attachment.gateway_proof.is_resolved() {
        return false;
    }
    candidate.spec.has_resolved_listener_attachment(&gateway.attachment)
}

fn select_gateway_edgion_tls_overlay(
    candidate: Option<Arc<CompiledEdgionTls>>,
    gateway: &GatewayTlsEntry,
) -> std::result::Result<Option<Arc<CompiledEdgionTls>>, ()> {
    let Some(candidate) = candidate else {
        return Ok(None);
    };
    if !edgion_tls_attaches_to_gateway(&candidate.tls, gateway) {
        return Ok(None);
    }
    if candidate.tls.spec.client_auth.is_some() {
        return Err(());
    }
    Ok(Some(candidate))
}

impl TlsCallback {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    /// Create TLS settings: registers `select_certificate_callback` (matching
    /// + snapshot) and the `TlsAccept` callbacks (cert_cb + handshake_complete_cb).
    pub fn new_tls_settings_with_callback(
        port: u16,
        listener_key: ListenerKey,
        egc: Arc<crate::config::GatewayConfigHandle>,
        enable_http2: bool,
    ) -> Result<TlsSettings> {
        let lk = listener_key;
        let callback = Box::new(TlsCallback::new(port));
        let mut settings =
            TlsSettings::with_callbacks(callback).map_err(|e| anyhow!("Failed to create TLS settings: {}", e))?;

        if enable_http2 {
            settings.enable_h2();
        }

        // Early callback: fires before BoringSSL freezes the version range
        // (boringssl/src/ssl/handshake_server.cc:710) — the only place
        // `set_min_proto_version` actually takes effect for this connection.
        //
        // Unified matching: performs full SNI resolution + Layer 1 (EdgionTls)
        // + Layer 2 (GatewayTls) here. On match, snapshot into SSL ex_data so
        // cert_cb is a pure consumer. On no-match, emit ssl.log + close the
        // connection immediately (before version negotiation).
        settings.deref_mut().set_select_certificate_callback(
            move |mut ch: ClientHello<'_>| -> std::result::Result<(), SelectCertError> {
                let gateway_config = egc.load();
                let mut ssl_ctx = SslCtx::new(port);

                // ── 1. SNI resolution ─────────────────────────────────────
                // Precedence: ClientHello SNI → Gateway-config fallback_sni.
                // If neither, we cannot pick a cert; emit ssl.log and close.
                let sni = if let Some(sni_val) = ch.servername(NameType::HOST_NAME) {
                    sni_val.to_lowercase()
                } else if let Some(fallback_sni) = gateway_config.fallback_sni.clone() {
                    let _ = ssl_ctx.meta.log.get_or_insert_with(LogBuffer::new).push("fallback SNI");
                    fallback_sni
                } else {
                    ssl_ctx.meta.err_log = Some("no SNI".to_string());
                    log_ssl(&ssl_ctx.meta);
                    return Err(SelectCertError::ERROR);
                };
                let sni = ssl_ctx.meta.sni.insert(sni).as_str();

                // Capture both matcher generations once. A Gateway frontend
                // client-auth decision owns the connection when present; an
                // EdgionTls certificate may overlay it only when its complete
                // logical attachment identifies that exact winning listener.
                let et_matcher_snapshot = get_tls_cert_matcher().snapshot();
                let gateway_snapshot = crate::runtime::matching::tls::get_gateway_tls_matcher().snapshot();
                let gw = gateway_snapshot.match_sni(&lk, sni);
                if let Some(gw) = gw
                    .as_ref()
                    .filter(|entry| entry.frontend_mtls != FrontendMtlsOutcome::NotConfigured)
                    .cloned()
                {
                    if let FrontendMtlsOutcome::StrictReject { reason } = gw.frontend_mtls {
                        ssl_ctx.meta.err_log = Some(reason.to_string());
                        log_ssl(&ssl_ctx.meta);
                        return Err(SelectCertError::ERROR);
                    }
                    let Ok(exact_overlay) = select_gateway_edgion_tls_overlay(
                        et_matcher_snapshot.match_attachment_sni(&gw.attachment, sni),
                        &gw,
                    ) else {
                        ssl_ctx.meta.err_log = Some("frontend mTLS conflict".to_string());
                        log_ssl(&ssl_ctx.meta);
                        return Err(SelectCertError::ERROR);
                    };
                    ssl_ctx.gateway_edgion_tls_overlay = exact_overlay;
                    ssl_ctx.matched_gateway_tls = Some(gw);
                } else if let Some(et) = et_matcher_snapshot
                    .match_eligible_sni(&lk, sni, |attachment| gateway_snapshot.listener_is_eligible(attachment))
                {
                    ssl_ctx.meta.is_mtls = et.tls.is_mtls_enabled();
                    ssl_ctx.matched_edgion_tls = Some(et);
                } else if let Some(gw) = gw {
                    ssl_ctx.matched_gateway_tls = Some(gw);
                } else {
                    ssl_ctx.meta.err_log = Some("cert not found".to_string());
                    log_ssl(&ssl_ctx.meta);
                    return Err(SelectCertError::ERROR);
                }

                // Resolve after identity selection, so local overrides replace
                // defaults before any setter can reject an ineffective default.
                // The EGC Arc pins this decision; SSL owns the applied settings
                // afterward, and cert_cb consumes only the identity snapshot.
                if let Err(reason) = apply_selected_downstream_policy(
                    ch.ssl_mut(),
                    &ssl_ctx,
                    &gateway_config.downstream_tls,
                    crate::tls::fips_profile::fips_policy_active(),
                ) {
                    ssl_ctx.meta.err_log = Some(reason.to_string());
                    log_ssl(&ssl_ctx.meta);
                    return Err(SelectCertError::ERROR);
                }
                // Failure already writes its audit and must prevent cert_cb.
                if !store_ssl_ctx(ch.ssl_mut(), &ssl_ctx) {
                    return Err(SelectCertError::ERROR);
                }
                Ok(())
            },
        );

        Ok(settings)
    }

    /// Stamp `tls_id` and store SslCtx into ex_data for handshake_complete_callback.
    ///
    /// On store failure, sets `err_log` so the caller can mirror the
    /// apply_*_cert failure pattern (check `err_log.is_some()` → `log_ssl`).
    /// Without this signal, the caller silently returns and the only audit
    /// trace is the partial entry emitted internally by `store_ssl_ctx`.
    fn finalize_ssl_ctx(&self, ssl: &mut SslRef, ssl_ctx: &mut SslCtx) {
        let rand: u64 = random();
        ssl_ctx.meta.tls_id = Some(format!("{:016x}-{:016x}", ssl_ctx.meta.ts, rand));
        if !store_ssl_ctx(ssl, ssl_ctx) {
            let _ = ssl_ctx
                .meta
                .log
                .get_or_insert_with(LogBuffer::new)
                .push("store ctx failed");
            ssl_ctx.meta.err_log = Some("finalize: store ctx failed".to_string());
        }
    }

    /// Apply certificate from EdgionTls resource
    ///
    /// Ordering rule (load-bearing for fail-closed, see tls-F002):
    /// every SSL-level setup that does NOT depend on cert/key state runs
    /// BEFORE `ssl_use_certificate` / `ssl_use_private_key`. Reason: this
    /// callback has no way to abort the handshake from the outside — pingora's
    /// `TlsAccept::certificate_callback` is a `()`-returning async fn and
    /// always reports success to BoringSSL. The only handshake-aborting effect
    /// available is "do not bind a cert/key", which leaves BoringSSL with no
    /// usable cert and forces handshake failure. If a security-boundary
    /// control (e.g. `spec.ciphers` policy) fails after cert/key are already
    /// bound, the early `return` only skips the log entry — the connection
    /// still completes on whatever default BoringSSL had inherited.
    ///
    /// TLS policy already succeeded in select_cb, whose error return aborts
    /// before version negotiation. Certificate setup cannot reapply defaults.
    ///
    /// The same rule applies to `spec.client_auth`: mTLS verify configuration
    /// is a security-boundary control (operator-required client cert), and
    /// `configure_mtls` only operates on client-side verify state
    /// (`set_verify_cert_store`, `set_verify_depth`, `set_verify`, optional
    /// verify callback) with no server cert/key dependency. Its branch
    /// therefore runs before cert/key are bound, so an Err inside
    /// `configure_mtls` (e.g. CA Secret missing, garbage CA PEM) actually
    /// aborts the handshake rather than only suppressing the audit log. If a
    /// future change adds a server cert read inside `configure_mtls` (e.g.
    /// SAN-mirror logic), this ordering decision must be revisited.
    fn apply_edgion_tls_cert(&self, ssl: &mut SslRef, compiled: &Arc<CompiledEdgionTls>, ssl_ctx: &mut SslCtx) {
        let edgion_tls = &compiled.tls;
        let ns = edgion_tls.metadata.namespace.as_deref().unwrap_or("-");
        let name = edgion_tls.metadata.name.as_deref().unwrap_or("-");
        // Read the operator log labels resolved once at `pre_parse` (ingest time)
        // and cached on the spec, so the handshake callback only Arc-clones — no
        // metadata scan on the per-connection path.
        let labels = edgion_tls
            .spec
            .resolved_log_labels
            .clone()
            .map(crate::LogLabels::from_resolved);
        ssl_ctx.meta.matched = Some(MatchedInfo {
            rsc: Rsc {
                kind: "EdgionTls".to_string(),
                ns: ns.to_string(),
                name: name.to_string(),
                labels,
            },
            section: None,
            sv: edgion_tls.get_sync_version(),
        });

        // Minimum version, ciphers and curves were applied once in select_cb.

        // mTLS client-cert verification is a security-boundary control
        // (operator-required client cert). `configure_mtls` only touches
        // client verify state and has no server cert/key dependency, so it
        // runs BEFORE cert/key are bound -- that way a CA Secret read failure
        // or a garbage CA PEM stamps `err_log` and `return`s with no cert
        // bound, and BoringSSL aborts the handshake. See tls-F011.
        if let Some(ref client_auth) = edgion_tls.spec.client_auth {
            if let Err(e) = self.configure_mtls(ssl, client_auth, compiled) {
                ssl_ctx.meta.err_log = Some(format!("mtls config: {e}"));
                return;
            }
        }

        // EdgionTls is installed directly on the per-connection SSL. Clear any
        // identity inherited from its context before consuming the selected
        // resource so an early failure cannot fall back to it. This clear must
        // stay ahead of the compiled-identity check below: a resource whose
        // identity failed to compile returns from here, and leaving an
        // inherited identity bound would break the fail-close contract (B0075).
        unsafe {
            boring_sys::SSL_certs_clear(ssl.as_ptr());
        }

        let certificate = match compiled.identity.as_ref() {
            Ok(certificate) => certificate,
            Err(category) => {
                ssl_ctx.meta.err_log = Some((*category).to_string());
                return;
            }
        };

        // Runtime expiry recheck — the certificate may have expired since the
        // resource was published. Both install paths now compare the same
        // cold-compiled Unix bounds with the same inclusive classifier (tls-F003).
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs().min(i64::MAX as u64) as i64);
        if let Some(error) = certificate.validity.validity_error(now) {
            ssl_ctx.meta.err_log = Some(error.to_string());
            return;
        }

        if !install_compiled_serving_identity(ssl, certificate) {
            ssl_ctx.meta.err_log = Some("set serving identity".to_string());
        }
    }

    /// Apply certificate from Gateway Listener TLS configuration (from Secret).
    ///
    /// Ordering rule (load-bearing for fail-closed, see tls-F002 and tls-F011):
    /// every SSL-level setup that does NOT depend on cert/key state runs BEFORE
    /// `apply_compiled_certificate_to_ssl` (which binds the selected cold-compiled
    /// identity on this path).
    /// `TlsAccept::certificate_callback` is a `()`-returning async fn that
    /// always reports success to BoringSSL, so the only handshake-aborting
    /// effect available from inside cert_cb is "do not bind a cert/key". If a
    /// security-boundary control (e.g. listener-level `frontendValidation`)
    /// fails after cert/key are already bound, the early `return` only skips
    /// the audit log — the connection still completes with verify mode never
    /// installed and no client cert ever requested.
    ///
    /// Concretely: `select_gateway_certificate` runs first (it reads only the
    /// ClientHello sigalg extension to pick RSA vs ECDSA cert); then
    /// `frontendValidation` (pure client-verify configuration, no server
    /// cert/key dependency); only then does `apply_compiled_certificate_to_ssl` bind
    /// cert/key. If a future change adds a server cert read inside
    /// `configure_gateway_frontend_mtls` (e.g. SAN-mirror logic), this
    /// ordering decision must be revisited.
    fn apply_gateway_tls_cert(&self, ssl: &mut SslRef, gateway_tls: &GatewayTlsEntry, ssl_ctx: &mut SslCtx) {
        ssl_ctx.meta.matched = Some(MatchedInfo {
            rsc: Rsc {
                kind: "Gateway".to_string(),
                ns: gateway_tls.attachment.gateway_namespace.clone(),
                name: gateway_tls.attachment.gateway_name.clone(),
                // GatewayTlsEntry is a derived listener struct without the full
                // Gateway metadata, so no operator log labels are available here.
                labels: None,
            },
            section: Some(gateway_tls.attachment.listener_name.clone()),
            sv: 0,
        });

        // Controller resolution is authoritative. A missing or partial inline
        // set fails closed; the Gateway must never bypass ReferenceGrant by
        // consulting its global SecretStore.
        let certificates = match &gateway_tls.compiled_serving_certificates {
            Some(certificates)
                if certificates.len() == gateway_tls.certificate_refs.len() && !certificates.is_empty() =>
            {
                certificates
            }
            _ => {
                ssl_ctx.meta.err_log = Some("serving certificate set incomplete".to_string());
                return;
            }
        };
        if ssl_ctx.meta.err_log.is_some() {
            return;
        }

        // BoringSSL keeps a single certificate slot per SSL, so for a dual-cert
        // listener (e.g. RSA + ECDSA) we must select the cert matching the
        // client's offered signature algorithms here — the same selection
        // BoringSSL/Envoy perform internally. With one configured cert this is a
        // no-op. A bad/missing selected cert fails the handshake (fail-closed)
        // rather than silently serving nothing.
        let Some(certificate) = self.select_gateway_certificate(ssl, certificates.as_ref()) else {
            ssl_ctx.meta.err_log = Some("no usable cert".to_string());
            return;
        };
        // Listener-level frontendValidation is a security-boundary control
        // (operator-required client cert). `configure_gateway_frontend_mtls`
        // only touches client verify state (X509 store, verify mode, verify
        // depth, optional verify callback) with no server cert/key
        // dependency, so it runs BEFORE `apply_compiled_certificate_to_ssl`
        // binds cert/key.
        // On Err we stamp `err_log` and `return` with no cert bound — that is
        // what aborts the handshake. The Ok branch keeps the `is_mtls`
        // stamping contract unchanged. See tls-F011.
        if gateway_tls.frontend_mtls != FrontendMtlsOutcome::NotConfigured {
            match self.configure_gateway_frontend_mtls(ssl, gateway_tls) {
                Err(e) => {
                    ssl_ctx.meta.err_log = Some(format!("frontend mTLS error: {}", e));
                    return;
                }
                Ok(verify_installed) => ssl_ctx.meta.is_mtls = verify_installed,
            }
        }

        self.apply_compiled_certificate_to_ssl(ssl, certificate, ssl_ctx);
        if ssl_ctx.meta.err_log.is_some() {}
    }

    /// Whether the connecting client advertised an ECDSA signature algorithm in
    /// its `signature_algorithms` extension (read after the ClientHello has been
    /// processed, i.e. in cert_cb).
    #[cfg(feature = "boringssl")]
    fn client_supports_ecdsa(&self, ssl: &SslRef) -> bool {
        unsafe {
            let ssl_ptr = ssl as *const SslRef as *const boring_sys::SSL;
            let mut out: *const u16 = std::ptr::null();
            let n = boring_sys::SSL_get0_peer_verify_algorithms(ssl_ptr, &mut out);
            if n == 0 || out.is_null() {
                return false;
            }
            std::slice::from_raw_parts(out, n)
                .iter()
                .any(|&sa| boring_sys::SSL_get_signature_algorithm_key_type(sa) == boring_sys::EVP_PKEY_EC)
        }
    }

    /// Whether the negotiated protocol version is TLS 1.3. Safe to call in
    /// cert_cb: the server has already finalized version negotiation (which runs
    /// before cert selection and before cipher negotiation), so `version2`
    /// reflects the negotiated version.
    #[cfg(feature = "boringssl")]
    fn negotiated_is_tls13(&self, ssl: &SslRef) -> bool {
        ssl.version2() == Some(pingora_core::tls::ssl::SslVersion::TLS1_3)
    }

    /// Select the cert/key Secret whose key type matches the connecting client.
    /// BoringSSL exposes a single cert slot per SSL, so a dual-cert listener
    /// (RSA + ECDSA) requires explicit selection — otherwise only one key type
    /// would ever be served.
    ///
    /// ECDSA is preferred only when (a) the negotiated version is TLS 1.3 and
    /// (b) the client advertised an ECDSA signature algorithm. The TLS 1.3 gate
    /// matters: in TLS 1.2 the certificate's key type also constrains the usable
    /// cipher suites (ECDHE-ECDSA vs ECDHE-RSA), so selecting ECDSA purely from
    /// signature_algorithms can produce a no-shared-cipher failure for a client
    /// that offered only ECDHE-RSA ciphers. TLS 1.3 decouples auth from the
    /// cipher, making sigalg-based selection safe. For TLS 1.2 we keep the
    /// legacy first-cert behavior (no regression versus the pre-fix code).
    #[cfg(feature = "boringssl")]
    fn select_gateway_certificate<'a>(
        &self,
        ssl: &SslRef,
        certificates: &'a [Arc<CompiledServingCertificate>],
    ) -> Option<&'a CompiledServingCertificate> {
        if certificates.len() <= 1 {
            return certificates.first().map(Arc::as_ref);
        }
        let prefer_ecdsa = self.negotiated_is_tls13(ssl) && self.client_supports_ecdsa(ssl);
        if prefer_ecdsa {
            if let Some(certificate) = certificates
                .iter()
                .find(|certificate| certificate.key_type == pingora_core::tls::pkey::Id::EC)
            {
                return Some(certificate.as_ref());
            }
        }
        certificates
            .iter()
            .find(|certificate| certificate.key_type != pingora_core::tls::pkey::Id::EC)
            .or_else(|| certificates.first())
            .map(Arc::as_ref)
    }

    /// Non-BoringSSL fallback: no per-key-type selection primitive is wired up,
    /// so serve the first configured cert (unchanged legacy behavior).
    #[cfg(not(feature = "boringssl"))]
    fn select_gateway_certificate<'a>(
        &self,
        _ssl: &SslRef,
        certificates: &'a [Arc<CompiledServingCertificate>],
    ) -> Option<&'a CompiledServingCertificate> {
        certificates.first().map(Arc::as_ref)
    }

    /// Install one immutable cold-compiled serving identity. No PEM parsing,
    /// key decoding, or secret copies occur on the handshake path.
    fn apply_compiled_certificate_to_ssl(
        &self,
        ssl: &mut SslRef,
        certificate: &CompiledServingCertificate,
        ssl_ctx: &mut SslCtx,
    ) {
        let Some(_leaf) = certificate.chain.first() else {
            ssl_ctx.meta.err_log = Some("serving certificate chain empty".to_string());
            return;
        };
        // Runtime expiry recheck — cert may have expired between controller
        // rebuilds. Mirrors the EdgionTls path in apply_edgion_tls_cert so that
        // GatewayTls-sourced certs emit the same cert_expired / cert_not_yet_valid
        // audit signal (RFC 5280 inclusive bounds). The `&now` of the EdgionTls
        // block is dropped here on purpose — clippy::op_ref flags it, and
        // `Asn1Time`/`Asn1TimeRef` cross-compare unchanged either way.
        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs().min(i64::MAX as u64) as i64);
        if let Some(error) = certificate.validity.validity_error(now_unix) {
            ssl_ctx.meta.err_log = Some(error.to_string());
            return;
        }
        #[cfg(feature = "boringssl")]
        {
            // The replacement contract is clear-then-set. Any failure clears
            // again, leaving the connection without a usable identity.
            if !install_compiled_serving_identity(ssl, certificate) {
                ssl_ctx.meta.err_log = Some("set serving identity".to_string());
            }
        }
        #[cfg(not(feature = "boringssl"))]
        {
            if let Err(e) = pingora_core::tls::ext::ssl_use_certificate(ssl, _leaf) {
                ssl_ctx.meta.err_log = Some(format!("set cert: {e}"));
                return;
            }
            for intermediate in certificate.chain.iter().skip(1) {
                if let Err(e) = pingora_core::tls::ext::ssl_add_chain_cert(ssl, intermediate) {
                    ssl_ctx.meta.err_log = Some(format!("set chain: {e}"));
                    return;
                }
            }
            if let Err(e) = pingora_core::tls::ext::ssl_use_private_key(ssl, &certificate.private_key) {
                ssl_ctx.meta.err_log = Some(format!("set key: {e}"));
            }
        }
    }

    /// Configure mTLS for Gateway standard frontendValidation from the matcher's
    /// immutable compiled outcome and CA store.
    ///
    /// Returns `Ok(false)` only for `NotConfigured`. A strict projection with
    /// empty, missing, incomplete, or invalid CA references is compiled as
    /// `StrictReject` and returns `Err`; it cannot become one-way TLS here.
    /// Usable strict and explicit insecure-fallback outcomes install a verify
    /// mode and return `Ok(true)`. The caller stamps `is_mtls` from this bool.
    fn configure_gateway_frontend_mtls(
        &self,
        ssl: &mut SslRef,
        gateway_tls: &GatewayTlsEntry,
    ) -> Result<bool, Box<PingoraError>> {
        match gateway_tls.frontend_mtls {
            FrontendMtlsOutcome::NotConfigured => return Ok(false),
            FrontendMtlsOutcome::StrictReject { reason } => {
                return Err(PingoraError::explain(ErrorType::InvalidCert, reason));
            }
            FrontendMtlsOutcome::StrictEffective | FrontendMtlsOutcome::InsecureFallback => {}
        }

        if gateway_tls.frontend_mtls == FrontendMtlsOutcome::StrictEffective {
            let store = gateway_tls
                .compiled_frontend_ca_store
                .as_ref()
                .ok_or_else(|| PingoraError::explain(ErrorType::InvalidCert, "frontend CA store unavailable"))?;
            ssl.set_verify_cert_store(store.as_ref().clone())
                .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Failed to set CA store: {}", e)))?;
        }

        if gateway_tls.frontend_mtls == FrontendMtlsOutcome::InsecureFallback {
            // InsecureFallback: request client cert but accept all results (including
            // invalid/missing certs). A plain SslVerifyMode::PEER still rejects invalid
            // certs, so we need a custom verify callback that always returns 1.
            #[cfg(feature = "boringssl")]
            unsafe {
                unsafe extern "C" fn insecure_fallback_verify_cb(
                    _preverify_ok: std::os::raw::c_int,
                    _ctx: *mut boring_sys::X509_STORE_CTX,
                ) -> std::os::raw::c_int {
                    1 // Always accept
                }
                let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;
                boring_sys::SSL_set_verify(
                    ssl_ptr,
                    SslVerifyMode::PEER.bits() as std::os::raw::c_int,
                    Some(insecure_fallback_verify_cb),
                );
            }
            #[cfg(not(feature = "boringssl"))]
            {
                // Without boringssl, no custom verify callback is available.
                // SslVerifyMode::PEER alone will still reject invalid certs.
                ssl.set_verify(SslVerifyMode::PEER);
            }
        } else {
            ssl.set_verify(SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT);
        }

        #[cfg(feature = "boringssl")]
        {
            ssl.set_verify_depth(10);
        }

        Ok(true)
    }

    /// Configure mTLS client certificate verification.
    fn configure_mtls(
        &self,
        ssl: &mut SslRef,
        client_auth: &ClientAuthConfig,
        compiled: &Arc<CompiledEdgionTls>,
    ) -> Result<(), Box<PingoraError>> {
        // Terminate mode presents the server certificate but never requests or
        // verifies a client certificate, so it needs no client CA store. Decide
        // the verify mode first and return early for Terminate: this avoids
        // failing the handshake for a valid Terminate config that has no
        // caSecretRef (the validator does not require one), and skips installing
        // a CA store that would never be consulted.
        let verify_mode = match client_auth.mode {
            ClientAuthMode::Terminate => return Ok(()),
            ClientAuthMode::Mutual => SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT,
            ClientAuthMode::OptionalMutual => SslVerifyMode::PEER,
        };

        // The trust store was built when this generation was published. Cloning
        // an `X509Store` is an `X509_STORE_up_ref` refcount bump, so the
        // handshake installs the shared store without rebuilding it — the same
        // idiom the Gateway listener path uses for `compiled_frontend_ca_store`.
        let store = match compiled.client_ca_store.as_ref() {
            Ok(Some(store)) => store,
            // Unreachable: this arm is gated on Mutual/OptionalMutual, for which
            // compilation always produces a store or an error. Fail closed
            // rather than proceeding with no trust store installed.
            Ok(None) => {
                return Err(PingoraError::explain(
                    ErrorType::InvalidCert,
                    "client CA store unavailable",
                ))
            }
            Err(e) => return Err(PingoraError::explain(ErrorType::InvalidCert, e.clone())),
        };

        ssl.set_verify_cert_store(store.as_ref().clone())
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Failed to set CA store: {}", e)))?;

        // verify_depth is validated to be in range 1–9 by the schema; u8 → u32 is lossless.
        #[cfg(feature = "boringssl")]
        {
            ssl.set_verify_depth(client_auth.verify_depth as u32);
        }
        #[cfg(not(feature = "boringssl"))]
        {
            let _ = client_auth.verify_depth;
        }

        if client_auth.allowed_sans.is_some() || client_auth.allowed_cns.is_some() {
            if let Err(e) = set_mtls_verify_callback(ssl, verify_mode, &compiled.tls) {
                return Err(PingoraError::explain(
                    ErrorType::InternalError,
                    format!("Failed to set verify callback: {}", e),
                ));
            }
        } else {
            ssl.set_verify(verify_mode);
        }

        Ok(())
    }
}

#[cfg(test)]
include!("../../../../tests/support/tls/runtime/gateway/tls_pingora_test/1196_function_item.rs");

#[cfg(all(test, feature = "boringssl"))]
#[path = "../../../../tests/unit/tls/runtime/gateway/tls_pingora_test.rs"]
mod tests;
