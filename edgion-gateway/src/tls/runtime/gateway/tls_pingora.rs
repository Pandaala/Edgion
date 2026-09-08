use crate::observe::logs::ssl_log::log_ssl;
use crate::observe::logs::LogBuffer;
use crate::runtime::matching::tls::CompiledServingCertificate;
use crate::runtime::matching::{match_gateway_tls, FrontendMtlsOutcome, GatewayTlsEntry};
use crate::tls::runtime::backend::cert_extractor::extract_client_cert_info;
use crate::tls::runtime::backend::set_mtls_verify_callback;
use crate::tls::store::cert_matcher::get_tls_cert_matcher;
use crate::{MatchedInfo, Rsc, TlsConnMeta};
use anyhow::anyhow;
use anyhow::Result;
use edgion_resources::listener_key::ListenerKey;
use edgion_resources::resources::edgion_gateway_config::EdgionGatewayConfig;
use edgion_resources::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode, EdgionTls};
use edgion_resources::ResourceMeta;
#[cfg(feature = "boringssl")]
use foreign_types::{ForeignType, ForeignTypeRef};
use pingora_core::listeners::tls::TlsSettings;
use pingora_core::listeners::TlsAccept;
use pingora_core::protocols::tls::TlsRef;
use pingora_core::tls::pkey::PKey;
use pingora_core::tls::ssl::{ClientHello, NameType, SelectCertError, SslRef, SslVerifyMode};
use pingora_core::tls::x509::store::X509StoreBuilder;
use pingora_core::tls::x509::X509;
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
    meta: TlsConnMeta,
    matched_edgion_tls: Option<Arc<EdgionTls>>,
    matched_gateway_tls: Option<Arc<GatewayTlsEntry>>,
    gateway_edgion_tls_overlay: Option<Arc<EdgionTls>>,
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

fn serving_certificate_validity_error(not_before: i64, not_after: i64, now: i64) -> Option<&'static str> {
    if now < not_before {
        Some("cert_not_yet_valid")
    } else if now > not_after {
        Some("cert_expired")
    } else {
        None
    }
}

impl SslCtx {
    fn new(port: u16) -> Self {
        Self {
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
    /// when `finalize_ssl_ctx` never ran (no `tls_id`), emits an ssl.log entry
    /// — covers the early-abort case where BoringSSL alerts during version
    /// negotiation between select_cb and cert_cb (typical: min_tls_version
    /// rejects the client).
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
            let ssl_ctx = Box::from_raw(ptr as *mut SslCtx);
            if ssl_ctx.meta.tls_id.is_none() {
                let mut meta = ssl_ctx.meta.clone();
                // Preserve a more specific upstream err_log (e.g. "min ver: …")
                // if select_cb already set one.
                if meta.err_log.is_none() {
                    meta.err_log = Some("early abort".to_string());
                }
                crate::observe::logs::ssl_log::log_ssl(&meta);
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

    /// Peek at TlsConnMeta in SSL ex_data without taking ownership.
    /// Used by mTLS verify callback to write ssl.log on failure.
    ///
    /// # Safety
    /// `ssl_ptr` must be a valid SSL pointer during the call.
    pub(super) unsafe fn peek_tls_conn_meta(ssl_ptr: *const boring_sys::SSL) -> Option<crate::ctx::TlsConnMeta> {
        let idx = SSL_CTX_EX_DATA_IDX.load(Ordering::Acquire);
        if idx < 0 {
            return None;
        }
        let ptr = boring_sys::SSL_get_ex_data(ssl_ptr, idx);
        if ptr.is_null() {
            return None;
        }
        let ctx = &*(ptr as *const SslCtx);
        Some(ctx.meta.clone())
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

/// Peek at TlsConnMeta stored in SSL ex_data without taking ownership.
/// Used by mTLS verify callback to write ssl.log on handshake failure.
///
/// # Safety
/// `ssl_ptr` must be a valid SSL pointer during the call.
#[cfg(feature = "boringssl")]
pub(crate) unsafe fn peek_tls_conn_meta(ssl_ptr: *const boring_sys::SSL) -> Option<crate::ctx::TlsConnMeta> {
    ssl_ctx_ex_data::peek_tls_conn_meta(ssl_ptr)
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
            ssl_ctx.matched_gateway_tls.clone(),
            ssl_ctx.gateway_edgion_tls_overlay.clone(),
        ) {
            match self.configure_gateway_frontend_mtls(ssl, &gateway_tls) {
                Err(e) => {
                    ssl_ctx.meta.err_log = Some(format!("frontend mTLS error: {e}"));
                    log_ssl(&ssl_ctx.meta);
                    return;
                }
                Ok(verify_installed) => ssl_ctx.meta.is_mtls = verify_installed,
            }
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
                if edgion_tls.is_mtls_enabled() && edgion_tls.should_expose_client_cert() {
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

/// Map EdgionTls `TlsVersion` (CRD enum) to BoringSSL `SslVersion`. Free fn
/// so the select_cb closure can call it without borrowing `&TlsCallback`.
fn map_tls_version(v: edgion_resources::resources::edgion_tls::TlsVersion) -> pingora_core::tls::ssl::SslVersion {
    use edgion_resources::resources::edgion_tls::TlsVersion;
    use pingora_core::tls::ssl::SslVersion;
    match v {
        TlsVersion::Tls10 => SslVersion::TLS1,
        TlsVersion::Tls11 => SslVersion::TLS1_1,
        TlsVersion::Tls12 => SslVersion::TLS1_2,
        TlsVersion::Tls13 => SslVersion::TLS1_3,
    }
}

fn edgion_tls_attaches_to_gateway(candidate: &EdgionTls, gateway: &GatewayTlsEntry) -> bool {
    if !gateway.attachment.gateway_proof.is_resolved() {
        return false;
    }
    candidate.spec.has_resolved_listener_attachment(&gateway.attachment)
}

fn select_gateway_edgion_tls_overlay(
    candidate: Option<Arc<EdgionTls>>,
    gateway: &GatewayTlsEntry,
) -> std::result::Result<Option<Arc<EdgionTls>>, ()> {
    let Some(candidate) = candidate else {
        return Ok(None);
    };
    if !edgion_tls_attaches_to_gateway(&candidate, gateway) {
        return Ok(None);
    }
    if candidate.spec.client_auth.is_some() {
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
        edgion_gateway_config: Arc<EdgionGatewayConfig>,
        enable_http2: bool,
    ) -> Result<TlsSettings> {
        let lk = listener_key;
        let gw_config = edgion_gateway_config;
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
                let mut ssl_ctx = SslCtx::new(port);

                // ── 1. SNI resolution ─────────────────────────────────────
                // Precedence: ClientHello SNI → Gateway-config fallback_sni.
                // If neither, we cannot pick a cert; emit ssl.log and close.
                let sni = if let Some(sni_val) = ch.servername(NameType::HOST_NAME) {
                    sni_val.to_lowercase()
                } else if let Some(fallback_sni) = gw_config
                    .spec
                    .security_protect
                    .as_ref()
                    .and_then(|sp| sp.fallback_sni.clone())
                {
                    let _ = ssl_ctx.meta.log.get_or_insert_with(LogBuffer::new).push("fallback SNI");
                    fallback_sni
                } else {
                    ssl_ctx.meta.err_log = Some("no SNI".to_string());
                    log_ssl(&ssl_ctx.meta);
                    return Err(SelectCertError::ERROR);
                };
                ssl_ctx.meta.sni = Some(sni.clone());

                // Capture both matcher generations once. A Gateway frontend
                // client-auth decision owns the connection when present; an
                // EdgionTls certificate may overlay it only when its complete
                // logical attachment identifies that exact winning listener.
                let et_matcher_snapshot = get_tls_cert_matcher().snapshot();
                let et_candidates = et_matcher_snapshot.match_sni_candidates(&lk, &sni);
                let et = et_candidates.first().cloned();
                let gw = match_gateway_tls(&lk, &sni);
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
                    let exact_overlay = match select_gateway_edgion_tls_overlay(
                        et_matcher_snapshot.match_attachment_sni(&gw.attachment, &sni),
                        &gw,
                    ) {
                        Ok(overlay) => overlay,
                        Err(()) => {
                            ssl_ctx.meta.err_log = Some("frontend mTLS conflict".to_string());
                            log_ssl(&ssl_ctx.meta);
                            return Err(SelectCertError::ERROR);
                        }
                    };
                    if let Some(candidate) = exact_overlay {
                        if let Some(min_version) = candidate.spec.min_tls_version {
                            if let Err(e) = ch.ssl_mut().set_min_proto_version(Some(map_tls_version(min_version))) {
                                ssl_ctx.meta.err_log = Some(format!("min ver: {e}"));
                                log_ssl(&ssl_ctx.meta);
                                return Err(SelectCertError::ERROR);
                            }
                        }
                        ssl_ctx.gateway_edgion_tls_overlay = Some(candidate);
                    }
                    ssl_ctx.matched_gateway_tls = Some(gw);
                    if !store_ssl_ctx(ch.ssl_mut(), &ssl_ctx) {
                        return Err(SelectCertError::ERROR);
                    }
                    return Ok(());
                }

                // ── 2. Layer 1: EdgionTls ────────────────────────────────
                if let Some(et) = et {
                    // Only Mutual / OptionalMutual actually drive mTLS verification.
                    // Terminate mode short-circuits in configure_mtls without setting
                    // any verify mode, so flagging is_mtls=true there would mislead
                    // ssl.log / tls.log / access_log consumers.
                    ssl_ctx.meta.is_mtls = et.is_mtls_enabled();
                    if let Some(min_version) = et.spec.min_tls_version {
                        if let Err(e) = ch.ssl_mut().set_min_proto_version(Some(map_tls_version(min_version))) {
                            ssl_ctx.meta.err_log = Some(format!("min ver: {e}"));
                            log_ssl(&ssl_ctx.meta);
                            return Err(SelectCertError::ERROR);
                        }
                    }
                    ssl_ctx.matched_edgion_tls = Some(et);
                    // store_ssl_ctx failure already emits its own ssl.log entry.
                    // Fail-closed here so cert_cb is never reached without a
                    // snapshot — preserves the "one ssl.log per connection"
                    // invariant.
                    if !store_ssl_ctx(ch.ssl_mut(), &ssl_ctx) {
                        return Err(SelectCertError::ERROR);
                    }
                    return Ok(());
                }

                // ── 3. Layer 2: GatewayTls ───────────────────────────────
                if let Some(gw) = gw {
                    ssl_ctx.matched_gateway_tls = Some(gw);
                    if !store_ssl_ctx(ch.ssl_mut(), &ssl_ctx) {
                        return Err(SelectCertError::ERROR);
                    }
                    return Ok(());
                }

                // ── 4. No match: emit audit log and close connection ─────
                ssl_ctx.meta.err_log = Some("cert not found".to_string());
                log_ssl(&ssl_ctx.meta);
                Err(SelectCertError::ERROR)
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
    /// Concretely: `spec.ciphers` is processed first. `SSL_set_strict_cipher_list`
    /// is pure SSL-level configuration with no cert dependency, so the early
    /// position is safe; and on failure we have not yet bound cert/key, so the
    /// `return` here is what actually fails the handshake closed.
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
    fn apply_edgion_tls_cert(&self, ssl: &mut SslRef, edgion_tls: &Arc<EdgionTls>, ssl_ctx: &mut SslCtx) {
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

        // Cipher policy is a security-boundary control (FIPS-only, exclude-weak,
        // …). BoringSSL's `SSL_set_strict_cipher_list` rejects unrecognized
        // names rather than applying them and leaves the SSL on its inherited
        // default suite — so on Err we MUST not bind cert/key, otherwise the
        // handshake completes negotiating from BoringSSL's full default and the
        // operator's restrictive policy is silently dropped. Doing this BEFORE
        // cert/key (instead of the historical "after" position) is what
        // converts the `err_log + return` pattern into an actual handshake
        // abort, since "no cert bound" is the only fail-close lever we have
        // from inside cert_cb (see the function-level doc above).
        // Cipher selection: operator `spec.ciphers` wins (the free per-workload
        // override); otherwise a FIPS build applies the approved default set;
        // otherwise BoringSSL's default is left in place. All three keep the
        // fail-close ordering: any Err returns before cert/key are bound, so the
        // handshake aborts rather than silently negotiating a wider policy.
        use crate::tls::fips_profile::{
            configure_fips_ciphers, configure_fips_curves, decide_cipher_action, fips_policy_active, CipherAction,
        };
        // FIPS enforcement is active on a real FIPS build OR when force-enabled
        // via config/env (fips_policy_active). When active, the approved-curve
        // restriction always applies, even if the operator overrides the cipher
        // list — curves are orthogonal to the cipher list, so a `spec.ciphers`
        // override must not silently re-enable a non-approved curve (e.g.
        // X25519).
        let enforce_fips = fips_policy_active();
        if enforce_fips {
            if let Err(e) = configure_fips_curves(ssl) {
                ssl_ctx.meta.err_log = Some(format!("fips curves config: {e}"));
                return;
            }
        }
        match decide_cipher_action(edgion_tls.spec.ciphers.is_some(), enforce_fips) {
            CipherAction::Operator => {
                if let Some(ref ciphers) = edgion_tls.spec.ciphers {
                    if let Err(e) = self.configure_ciphers(ssl, ciphers) {
                        ssl_ctx.meta.err_log = Some(format!("cipher config: {e}"));
                        return;
                    }
                }
            }
            CipherAction::FipsDefault => {
                if let Err(e) = configure_fips_ciphers(ssl) {
                    ssl_ctx.meta.err_log = Some(format!("fips cipher config: {e}"));
                    return;
                }
            }
            CipherAction::BoringDefault => {}
        }

        // mTLS client-cert verification is a security-boundary control
        // (operator-required client cert). `configure_mtls` only touches
        // client verify state and has no server cert/key dependency, so it
        // runs BEFORE cert/key are bound -- that way a CA Secret read failure
        // or a garbage CA PEM stamps `err_log` and `return`s with no cert
        // bound, and BoringSSL aborts the handshake. See tls-F011.
        if let Some(ref client_auth) = edgion_tls.spec.client_auth {
            if let Err(e) = self.configure_mtls(ssl, client_auth, edgion_tls) {
                ssl_ctx.meta.err_log = Some(format!("mtls config: {e}"));
                return;
            }
        }

        let cert_pem = match edgion_tls.cert_pem() {
            Ok(pem) => pem,
            Err(e) => {
                ssl_ctx.meta.err_log = Some(format!("cert read: {e}"));
                return;
            }
        };
        let cert = match X509::from_pem(cert_pem.as_bytes()) {
            Ok(c) => c,
            Err(e) => {
                ssl_ctx.meta.err_log = Some(format!("bad cert: {e}"));
                return;
            }
        };
        // Runtime expiry recheck — cert may have expired after load. Mirrors
        // validate_cert/check_expiration semantics (RFC 5280 inclusive bounds).
        #[cfg(feature = "boringssl")]
        match boring::asn1::Asn1Time::days_from_now(0) {
            Ok(now) => {
                if cert.not_after() < &now {
                    ssl_ctx.meta.err_log = Some(format!("cert_expired: not_after={}", cert.not_after()));
                    return;
                }
                if cert.not_before() > &now {
                    ssl_ctx.meta.err_log = Some(format!("cert_not_yet_valid: not_before={}", cert.not_before()));
                    return;
                }
            }
            Err(_e) => {
                // days_from_now(0) only fails on BoringSSL allocation failure
                // (effectively impossible). Don't fail the connection — the
                // cert was validated at load time and BoringSSL rejects
                // genuinely broken certs during the handshake.
                let _ = ssl_ctx
                    .meta
                    .log
                    .get_or_insert_with(crate::observe::logs::LogBuffer::new)
                    .push_err("cert_expiry_check_failed");
            }
        }
        if let Err(e) = pingora_core::tls::ext::ssl_use_certificate(ssl, &cert) {
            ssl_ctx.meta.err_log = Some(format!("set cert: {e}"));
            return;
        }

        let key_pem = match edgion_tls.key_pem() {
            Ok(pem) => pem,
            Err(e) => {
                ssl_ctx.meta.err_log = Some(format!("key read: {e}"));
                return;
            }
        };
        let key = match PKey::private_key_from_pem(key_pem.as_bytes()) {
            Ok(k) => k,
            Err(e) => {
                ssl_ctx.meta.err_log = Some(format!("bad key: {e}"));
                return;
            }
        };
        if let Err(e) = pingora_core::tls::ext::ssl_use_private_key(ssl, &key) {
            ssl_ctx.meta.err_log = Some(format!("set key: {e}"));
            return;
        }
    }

    /// Apply certificate from Gateway Listener TLS configuration (from Secret).
    ///
    /// Ordering rule (load-bearing for fail-closed, see tls-F002 and tls-F011):
    /// every SSL-level setup that does NOT depend on cert/key state runs BEFORE
    /// `apply_secret_to_ssl` (which is what binds cert/key on this path).
    /// `TlsAccept::certificate_callback` is a `()`-returning async fn that
    /// always reports success to BoringSSL, so the only handshake-aborting
    /// effect available from inside cert_cb is "do not bind a cert/key". If a
    /// security-boundary control (e.g. listener-level `frontendValidation`)
    /// fails after cert/key are already bound, the early `return` only skips
    /// the audit log — the connection still completes with verify mode never
    /// installed and no client cert ever requested.
    ///
    /// Concretely: `select_gateway_secret` runs first (it reads only the
    /// ClientHello sigalg extension to pick RSA vs ECDSA cert); then
    /// `frontendValidation` (pure client-verify configuration, no server
    /// cert/key dependency); only then does `apply_secret_to_ssl` bind
    /// cert/key. If a future change adds a server cert read inside
    /// `configure_gateway_frontend_mtls` (e.g. SAN-mirror logic), this
    /// ordering decision must be revisited.
    fn apply_gateway_tls_cert(&self, ssl: &mut SslRef, gateway_tls: &GatewayTlsEntry, ssl_ctx: &mut SslCtx) {
        ssl_ctx.meta.matched = Some(MatchedInfo {
            rsc: Rsc {
                kind: "Gateway".to_string(),
                ns: gateway_tls.gateway_namespace.clone(),
                name: gateway_tls.gateway_name.clone(),
                // GatewayTlsEntry is a derived listener struct without the full
                // Gateway metadata, so no operator log labels are available here.
                labels: None,
            },
            section: Some(gateway_tls.listener_name.clone()),
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
        let certificate = match self.select_gateway_certificate(ssl, certificates.as_slice()) {
            Some(certificate) => certificate,
            None => {
                ssl_ctx.meta.err_log = Some("no usable cert".to_string());
                return;
            }
        };
        // Listener-level frontendValidation is a security-boundary control
        // (operator-required client cert). `configure_gateway_frontend_mtls`
        // only touches client verify state (X509 store, verify mode, verify
        // depth, optional verify callback) with no server cert/key
        // dependency, so it runs BEFORE `apply_secret_to_ssl` binds cert/key.
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

        // FIPS: a FIPS build defaults this listener to the approved cipher/curve
        // set, matching the EdgionTls path. The Gateway API listener path has no
        // operator cipher field, so both the approved curves and the approved
        // cipher list always apply on a FIPS build (BoringSSL default
        // otherwise). Runs BEFORE apply_secret_to_ssl binds cert/key, so a
        // rejected cipher/curve aborts the handshake (fail-closed) rather than
        // negotiating a wider policy.
        if crate::tls::fips_profile::fips_policy_active() {
            if let Err(e) = crate::tls::fips_profile::configure_fips_curves(ssl) {
                ssl_ctx.meta.err_log = Some(format!("fips curves config: {e}"));
                return;
            }
            if let Err(e) = crate::tls::fips_profile::configure_fips_ciphers(ssl) {
                ssl_ctx.meta.err_log = Some(format!("fips cipher config: {e}"));
                return;
            }
        }

        self.apply_compiled_certificate_to_ssl(ssl, certificate, ssl_ctx);
        if ssl_ctx.meta.err_log.is_some() {
            return;
        }
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
        if let Some(error) = serving_certificate_validity_error(
            certificate.leaf_not_before_unix,
            certificate.leaf_not_after_unix,
            now_unix,
        ) {
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

    /// Configure mTLS for Gateway standard frontendValidation.
    /// Uses resolved CA secrets to verify client certificates.
    ///
    /// Returns `Ok(true)` when a verify mode was actually installed, `Ok(false)`
    /// when the listener has frontendValidation but no resolved CA secrets (so no
    /// client cert is requested/verified). The caller stamps `is_mtls` from this
    /// so the flag reflects the real handshake, not merely an Ok return.
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
        edgion_tls: &Arc<EdgionTls>,
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

        let ca_pem = edgion_tls.ca_cert_pem().map_err(|e| {
            PingoraError::explain(
                ErrorType::InvalidCert,
                format!("Failed to get CA cert PEM for mTLS: {}", e),
            )
        })?;

        let mut store_builder = X509StoreBuilder::new().map_err(|e| {
            PingoraError::explain(ErrorType::InvalidCert, format!("Failed to create X509 store: {}", e))
        })?;

        add_ca_bundle_to_store(&mut store_builder, ca_pem.as_bytes())?;

        let store = store_builder.build();

        ssl.set_verify_cert_store(store)
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
            if let Err(e) = set_mtls_verify_callback(ssl, verify_mode, edgion_tls) {
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

    /// Configure cipher list (similar to Nginx ssl_ciphers directive)
    ///
    /// Takes a list of cipher names in OpenSSL format and applies them via BoringSSL FFI.
    /// Returns `Err` if the cipher string is invalid or rejected by BoringSSL.
    ///
    /// Note: TLS 1.3 ciphers are hardcoded in BoringSSL and cannot be configured.
    fn configure_ciphers(&self, ssl: &mut SslRef, ciphers: &[String]) -> Result<(), Box<PingoraError>> {
        if ciphers.is_empty() {
            return Ok(());
        }

        #[cfg(feature = "boringssl")]
        {
            use std::ffi::CString;

            let cipher_list = ciphers.join(":");
            let cipher_cstr = CString::new(cipher_list.as_str())
                .map_err(|e| PingoraError::explain(ErrorType::InternalError, format!("cipher null byte: {e}")))?;

            // SAFETY: SslRef -> SSL* conversion for FFI call
            // ssl is valid during this function, cipher_cstr lifetime extends past FFI call
            unsafe {
                let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;
                let ret = boring_sys::SSL_set_strict_cipher_list(ssl_ptr, cipher_cstr.as_ptr());

                if ret != 1 {
                    return Err(PingoraError::explain(
                        ErrorType::InternalError,
                        format!("rejected ciphers: {}", cipher_list),
                    ));
                }
            }
        }

        #[cfg(not(feature = "boringssl"))]
        {
            let _ = ssl;
        }

        Ok(())
    }
}

/// Parse a (possibly multi-block) CA bundle PEM and add every certificate to
/// the store, returning the number added. RFC 5280 §6.1 path validation needs
/// the complete intermediate chain up to a trust anchor, so all PEM blocks must
/// be loaded — `X509::from_pem` reads only the first block. An empty or
/// unparseable bundle is rejected (fail-closed) so a misconfigured CA secret
/// cannot silently produce an empty trust store.
fn add_ca_bundle_to_store(store_builder: &mut X509StoreBuilder, ca_pem: &[u8]) -> Result<usize, Box<PingoraError>> {
    let bundle = edgion_common::tls_validation::parse_canonical_ca_bundle(ca_pem)
        .map_err(|error| PingoraError::explain(ErrorType::InvalidCert, format!("Invalid CA bundle: {error:?}")))?;
    let count = bundle.certificates_der.len();
    for der in bundle.certificates_der {
        let cert = X509::from_der(&der)
            .map_err(|error| PingoraError::explain(ErrorType::InvalidCert, format!("Invalid CA DER: {error}")))?;
        store_builder.add_cert(cert).map_err(|e| {
            PingoraError::explain(ErrorType::InvalidCert, format!("Failed to add CA cert to store: {}", e))
        })?;
    }
    Ok(count)
}

/// Pure selection rule shared by `select_gateway_secret`: given each secret's
/// key-type classification (`Some(true)` = ECDSA, `Some(false)` = RSA/other,
/// `None` = unparseable) and whether ECDSA is preferred, return the index of the
/// cert to serve. Prefers ECDSA when requested and available; otherwise the
/// first RSA cert; otherwise the first secret. Returns `None` only for an empty
/// list. Extracted as a free fn so the rule is unit-testable without an `SSL`.
#[cfg(all(test, feature = "boringssl"))]
fn pick_secret_index(kinds: &[Option<bool>], prefer_ecdsa: bool) -> Option<usize> {
    if kinds.is_empty() {
        return None;
    }
    if prefer_ecdsa {
        if let Some(i) = kinds.iter().position(|k| *k == Some(true)) {
            return Some(i);
        }
    }
    Some(kinds.iter().position(|k| *k == Some(false)).unwrap_or(0))
}

#[cfg(all(test, feature = "boringssl"))]
mod tests {
    use super::{
        install_compiled_serving_identity, pick_secret_index, select_gateway_edgion_tls_overlay,
        serving_certificate_validity_error,
    };
    use crate::runtime::matching::{FrontendMtlsOutcome, GatewayTlsEntry};
    use edgion_resources::resources::edgion_tls::{
        ClientAuthConfig, ClientAuthMode, EdgionTls, EdgionTlsSpec, ResolvedListenerAttachment,
    };
    use edgion_resources::resources::gateway::SecretObjectReference;
    use edgion_resources::ListenerKey;
    use kube::api::ObjectMeta;
    use std::net::SocketAddr;
    use std::sync::Arc;

    static HANDSHAKE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const RSA: Option<bool> = Some(false);
    const ECDSA: Option<bool> = Some(true);
    const UNKNOWN: Option<bool> = None;

    #[test]
    fn compiled_certificate_validity_bounds_are_inclusive() {
        assert_eq!(
            serving_certificate_validity_error(10, 20, 9),
            Some("cert_not_yet_valid")
        );
        assert_eq!(serving_certificate_validity_error(10, 20, 10), None);
        assert_eq!(serving_certificate_validity_error(10, 20, 20), None);
        assert_eq!(serving_certificate_validity_error(10, 20, 21), Some("cert_expired"));
    }

    fn overlay_fixture(attached_gateway: &str, client_auth: bool) -> (Arc<EdgionTls>, GatewayTlsEntry) {
        let listener_key = ListenerKey::new("0.0.0.0:443".parse::<SocketAddr>().unwrap(), "HTTPS");
        let gateway_proof = edgion_resources::resources::edgion_tls::GatewayAttachmentProof {
            instance: "gateway-uid".to_string(),
            generation: 7,
            epoch: "resource-version-9".to_string(),
            attachment_digest: "a".repeat(64),
        };
        let tls = Arc::new(EdgionTls {
            metadata: ObjectMeta {
                namespace: Some("default".to_string()),
                name: Some("tls".to_string()),
                ..Default::default()
            },
            spec: EdgionTlsSpec {
                parent_refs: None,
                hosts: vec!["example.com".to_string()],
                secret_ref: SecretObjectReference {
                    group: None,
                    kind: None,
                    name: "server".to_string(),
                    namespace: None,
                },
                client_auth: client_auth.then_some(ClientAuthConfig {
                    mode: ClientAuthMode::Mutual,
                    ca_secret_ref: None,
                    verify_depth: 1,
                    ca_secret: None,
                    allowed_sans: None,
                    allowed_cns: None,
                }),
                min_tls_version: None,
                ciphers: None,
                secret: None,
                resolved_listeners: Some(vec![listener_key.clone()]),
                resolved_listener_attachments: Some(vec![ResolvedListenerAttachment {
                    gateway_namespace: "default".to_string(),
                    gateway_name: attached_gateway.to_string(),
                    listener_name: "https".to_string(),
                    listener_key: listener_key.clone(),
                    gateway_proof: gateway_proof.clone(),
                }]),
                resolved_listener_attachment_index: Default::default(),
                resolved_log_labels: None,
            },
            status: None,
        });
        let gateway = GatewayTlsEntry {
            attachment: ResolvedListenerAttachment {
                gateway_namespace: "default".to_string(),
                gateway_name: "winner".to_string(),
                listener_name: "https".to_string(),
                listener_key: listener_key.clone(),
                gateway_proof: gateway_proof.clone(),
            },
            gateway_namespace: "default".to_string(),
            gateway_name: "winner".to_string(),
            gateway_creation_ts: None,
            listener_name: "https".to_string(),
            listener_key,
            gateway_proof,
            hostname: "example.com".to_string(),
            certificate_refs: Vec::new(),
            compiled_serving_certificates: None,
            compiled_frontend_ca_store: None,
            frontend_mtls: FrontendMtlsOutcome::StrictEffective,
        };
        (tls, gateway)
    }

    #[test]
    fn gateway_overlay_requires_exact_attachment_and_rejects_dual_client_auth() {
        let (exact, gateway) = overlay_fixture("winner", false);
        assert!(select_gateway_edgion_tls_overlay(Some(exact.clone()), &gateway)
            .unwrap()
            .is_none());
        exact.spec.warm_listener_attachment_index();
        assert!(select_gateway_edgion_tls_overlay(Some(exact.clone()), &gateway)
            .unwrap()
            .is_some());

        let (mismatch, _) = overlay_fixture("other", false);
        mismatch.spec.warm_listener_attachment_index();
        assert!(select_gateway_edgion_tls_overlay(Some(mismatch), &gateway)
            .unwrap()
            .is_none());

        let (conflict, _) = overlay_fixture("winner", true);
        conflict.spec.warm_listener_attachment_index();
        assert!(select_gateway_edgion_tls_overlay(Some(conflict), &gateway).is_err());

        let mut stale_value = exact.as_ref().clone();
        stale_value.spec.resolved_listener_attachments.as_mut().unwrap()[0]
            .gateway_proof
            .epoch = "stale-resource-version".to_string();
        let stale = Arc::new(stale_value);
        stale.spec.warm_listener_attachment_index();
        assert!(select_gateway_edgion_tls_overlay(Some(stale), &gateway)
            .unwrap()
            .is_none());

        let selected = select_gateway_edgion_tls_overlay(Some(exact.clone()), &gateway)
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&selected, &exact));
    }

    #[test]
    fn published_regex_san_preserves_attachment_and_rejects_dual_client_auth() {
        use crate::tls::store::TlsStore;
        use edgion_resources::constants::secret_keys::tls::{CERT, KEY};
        use edgion_resources::resources::edgion_tls::{AllowedSan, AllowedSanMatch, AllowedSanType};
        use k8s_openapi::api::core::v1::Secret;
        use k8s_openapi::ByteString;
        use rcgen::{CertificateParams, KeyPair};
        use std::collections::{BTreeMap, HashMap, HashSet};

        let (fixture, gateway) = overlay_fixture("winner", true);
        let mut tls = (*fixture).clone();
        let key = KeyPair::generate().unwrap();
        let cert = CertificateParams::new(vec!["example.com".to_string()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        tls.spec.secret = Some(Secret {
            data: Some(BTreeMap::from([
                (CERT.to_string(), ByteString(cert.pem().into_bytes())),
                (KEY.to_string(), ByteString(key.serialize_pem().into_bytes())),
            ])),
            ..Default::default()
        });
        let mut san = AllowedSan::exact(AllowedSanType::Dns, r"^client\.example\.com$");
        san.match_kind = AllowedSanMatch::RegularExpression;
        let client_auth = tls.spec.client_auth.as_mut().unwrap();
        client_auth.allowed_sans = Some(vec![san]);
        client_auth.ca_secret_ref = Some(SecretObjectReference {
            name: "client-ca".to_string(),
            ..tls.spec.secret_ref.clone()
        });

        let store = TlsStore::new();
        for phase in ["full", "add", "update"] {
            let resource = Arc::new(tls.clone());
            let data = HashMap::from([("default/tls".to_string(), resource)]);
            match phase {
                "full" => store.replace_all_tls(data),
                "add" => {
                    store.replace_all_tls(HashMap::new());
                    store.apply_partial_update(data, HashMap::new(), &HashSet::new());
                }
                "update" => store.apply_partial_update(HashMap::new(), data, &HashSet::new()),
                _ => unreachable!(),
            }
            assert!(store.get_validation_status("default/tls").unwrap().is_valid);
            let published = store
                .snapshot()
                .match_attachment_sni(&gateway.attachment, "example.com")
                .expect("valid published certificate must match its exact attachment");
            let san = &published
                .spec
                .client_auth
                .as_ref()
                .unwrap()
                .allowed_sans
                .as_ref()
                .unwrap()[0];
            assert!(san.compiled_regex.is_some(), "SAN must compile during {phase}");
            // Do not manually warm the candidate: publication owns this cache.
            assert!(
                select_gateway_edgion_tls_overlay(Some(published), &gateway).is_err(),
                "dual client-auth must reject after {phase} publication"
            );
        }
    }

    #[test]
    fn empty_list_returns_none() {
        assert_eq!(pick_secret_index(&[], true), None);
        assert_eq!(pick_secret_index(&[], false), None);
    }

    #[cfg(feature = "boringssl")]
    #[test]
    fn strict_serving_parsers_preserve_chain_and_accept_supported_key_formats() {
        use boring::asn1::{Asn1Integer, Asn1Time};
        use boring::bn::BigNum;
        use boring::ec::{EcGroup, EcKey};
        use boring::hash::MessageDigest;
        use boring::nid::Nid;
        use boring::pkey::{PKey, Private};
        use boring::rsa::Rsa;
        use boring::x509::{X509NameBuilder, X509};

        fn certificate(
            cn: &str,
            subject_key: &PKey<Private>,
            issuer: Option<&X509>,
            signing_key: &PKey<Private>,
            serial: u32,
        ) -> X509 {
            let mut name = X509NameBuilder::new().unwrap();
            name.append_entry_by_text("CN", cn).unwrap();
            let name = name.build();
            let mut builder = X509::builder().unwrap();
            builder.set_version(2).unwrap();
            let serial = Asn1Integer::from_bn(&BigNum::from_u32(serial).unwrap()).unwrap();
            builder.set_serial_number(&serial).unwrap();
            builder.set_subject_name(&name).unwrap();
            builder
                .set_issuer_name(issuer.map_or(name.as_ref(), |certificate| certificate.subject_name()))
                .unwrap();
            builder.set_pubkey(subject_key).unwrap();
            builder.set_not_before(&Asn1Time::days_from_now(0).unwrap()).unwrap();
            builder.set_not_after(&Asn1Time::days_from_now(30).unwrap()).unwrap();
            builder.sign(signing_key, MessageDigest::sha256()).unwrap();
            builder.build()
        }

        let root_key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
        let root = certificate("root", &root_key, None, &root_key, 1);
        let leaf_rsa = Rsa::generate(2048).unwrap();
        let rsa_pkcs1 = String::from_utf8(leaf_rsa.private_key_to_pem().unwrap()).unwrap();
        let leaf_key = PKey::from_rsa(leaf_rsa).unwrap();
        let rsa_pkcs8 = String::from_utf8(leaf_key.private_key_to_pem_pkcs8().unwrap()).unwrap();
        let leaf = certificate("leaf", &leaf_key, Some(&root), &root_key, 2);
        let chain_pem = format!(
            "{}{}",
            String::from_utf8(leaf.to_pem().unwrap()).unwrap(),
            String::from_utf8(root.to_pem().unwrap()).unwrap()
        );
        let parsed = edgion_common::tls_validation::parse_strict_certificate_chain(chain_pem.as_bytes()).unwrap();
        assert_eq!(
            parsed.certificates_der,
            vec![leaf.to_der().unwrap(), root.to_der().unwrap()]
        );
        edgion_common::tls_validation::check_key_matches_cert(&chain_pem, &rsa_pkcs8).unwrap();
        edgion_common::tls_validation::check_key_matches_cert(&chain_pem, &rsa_pkcs1).unwrap();

        let ec = EcKey::generate(&EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap()).unwrap();
        let ec_sec1 = String::from_utf8(ec.private_key_to_pem().unwrap()).unwrap();
        let ec_key = PKey::from_ec_key(ec).unwrap();
        let ec_cert = certificate("ec-leaf", &ec_key, None, &ec_key, 3);
        let ec_cert_pem = String::from_utf8(ec_cert.to_pem().unwrap()).unwrap();
        edgion_common::tls_validation::check_key_matches_cert(&ec_cert_pem, &ec_sec1).unwrap();
    }

    #[test]
    fn atomic_identity_rotation_sends_intermediate_and_replaces_old_chain() {
        let _serial = HANDSHAKE_TEST_LOCK.lock().unwrap();
        use crate::runtime::matching::tls::{CompiledBoringCertificateChain, CompiledServingCertificate};
        use boring::pkey::PKey;
        use boring::ssl::{Ssl, SslContext, SslMethod, SslVerifyMode};
        use boring::x509::store::X509StoreBuilder;
        use boring::x509::X509;
        use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
        use std::net::{TcpListener, TcpStream};
        use std::time::Duration;

        fn identity(prefix: &str) -> (Arc<CompiledServingCertificate>, X509) {
            let root_key = KeyPair::generate().unwrap();
            let mut root_params = CertificateParams::default();
            root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            root_params
                .distinguished_name
                .push(rcgen::DnType::CommonName, format!("{prefix}-root"));
            let root_issuer = Issuer::from_params(&root_params, &root_key);
            let root = root_params.self_signed(&root_key).unwrap();

            let intermediate_key = KeyPair::generate().unwrap();
            let mut intermediate_params = CertificateParams::default();
            intermediate_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            intermediate_params
                .distinguished_name
                .push(rcgen::DnType::CommonName, format!("{prefix}-intermediate"));
            let intermediate_issuer = Issuer::from_params(&intermediate_params, &intermediate_key);
            let intermediate = intermediate_params.signed_by(&intermediate_key, &root_issuer).unwrap();

            let leaf_key = KeyPair::generate().unwrap();
            let leaf = CertificateParams::new(vec!["localhost".to_string()])
                .unwrap()
                .signed_by(&leaf_key, &intermediate_issuer)
                .unwrap();
            let chain_der = vec![leaf.der().to_vec(), intermediate.der().to_vec()];
            let chain = chain_der
                .iter()
                .map(|der| X509::from_der(der).unwrap())
                .collect::<Vec<_>>();
            let private_key = PKey::private_key_from_pem(leaf_key.serialize_pem().as_bytes()).unwrap();
            let compiled = Arc::new(CompiledServingCertificate {
                chain: Arc::new(chain),
                atomic_chain: Arc::new(CompiledBoringCertificateChain::from_der(&chain_der).unwrap()),
                key_type: private_key.id(),
                private_key,
                leaf_not_before_unix: i64::MIN,
                leaf_not_after_unix: i64::MAX,
            });
            (compiled, X509::from_der(root.der()).unwrap())
        }

        let (old_identity, old_root) = identity("old");
        let (new_identity, new_root) = identity("new");
        let invalid_replacement = Arc::new(CompiledServingCertificate {
            chain: old_identity.chain.clone(),
            atomic_chain: Arc::new(CompiledBoringCertificateChain::from_der(&[b"not-der".to_vec()]).unwrap()),
            private_key: old_identity.private_key.clone(),
            key_type: old_identity.key_type,
            leaf_not_before_unix: i64::MIN,
            leaf_not_after_unix: i64::MAX,
        });
        let server_ctx = SslContext::builder(SslMethod::tls()).unwrap().build();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut ssl = Ssl::new(&server_ctx).unwrap();
            assert!(install_compiled_serving_identity(ssl.as_mut(), &old_identity));
            assert!(install_compiled_serving_identity(ssl.as_mut(), &new_identity));
            let valid_rotation = ssl.accept(tcp).is_ok();

            let (tcp, _) = listener.accept().unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut ssl = Ssl::new(&server_ctx).unwrap();
            assert!(install_compiled_serving_identity(ssl.as_mut(), &old_identity));
            assert!(!install_compiled_serving_identity(ssl.as_mut(), &invalid_replacement));
            (valid_rotation, ssl.accept(tcp).is_err())
        });

        let mut store = X509StoreBuilder::new().unwrap();
        store.add_cert(new_root).unwrap();
        let mut client_builder = SslContext::builder(SslMethod::tls()).unwrap();
        client_builder.set_verify(SslVerifyMode::PEER);
        client_builder.set_verify_cert_store(store.build()).unwrap();
        let client_ctx = client_builder.build();
        let tcp = TcpStream::connect(address).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut client_ssl = Ssl::new(&client_ctx).unwrap();
        client_ssl.set_hostname("localhost").unwrap();
        let client = client_ssl.connect(tcp);
        assert!(
            client.is_ok(),
            "client trusting only the new root must validate the sent intermediate"
        );
        let mut old_store = X509StoreBuilder::new().unwrap();
        old_store.add_cert(old_root).unwrap();
        let mut old_client_builder = SslContext::builder(SslMethod::tls()).unwrap();
        old_client_builder.set_verify(SslVerifyMode::PEER);
        old_client_builder.set_verify_cert_store(old_store.build()).unwrap();
        let tcp = TcpStream::connect(address).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut old_client_ssl = Ssl::new(&old_client_builder.build()).unwrap();
        old_client_ssl.set_hostname("localhost").unwrap();
        assert!(
            old_client_ssl.connect(tcp).is_err(),
            "failed replacement must not continue serving the old identity"
        );
        let (valid_rotation, failed_replacement) = server.join().unwrap();
        assert!(valid_rotation);
        assert!(failed_replacement);
    }

    #[test]
    fn ecdsa_preferred_picks_ecdsa_regardless_of_order() {
        assert_eq!(pick_secret_index(&[RSA, ECDSA], true), Some(1));
        assert_eq!(pick_secret_index(&[ECDSA, RSA], true), Some(0));
    }

    #[test]
    fn ecdsa_not_preferred_picks_first_rsa() {
        assert_eq!(pick_secret_index(&[ECDSA, RSA], false), Some(1));
        assert_eq!(pick_secret_index(&[RSA, ECDSA], false), Some(0));
    }

    #[test]
    fn ecdsa_preferred_but_only_rsa_falls_back_to_rsa() {
        assert_eq!(pick_secret_index(&[RSA, RSA], true), Some(0));
    }

    #[test]
    fn no_rsa_present_falls_back_to_first() {
        // prefer_ecdsa false but only ECDSA certs: no Some(false), fall to index 0.
        assert_eq!(pick_secret_index(&[ECDSA, ECDSA], false), Some(0));
    }

    #[test]
    fn unparseable_certs_are_skipped_then_fall_back() {
        // ECDSA preferred, first is unknown, second is ECDSA.
        assert_eq!(pick_secret_index(&[UNKNOWN, ECDSA], true), Some(1));
        // RSA preferred path skips unknown, picks the RSA.
        assert_eq!(pick_secret_index(&[UNKNOWN, RSA], false), Some(1));
        // All unknown: fall back to first.
        assert_eq!(pick_secret_index(&[UNKNOWN, UNKNOWN], true), Some(0));
    }

    #[test]
    fn single_cert_is_index_zero() {
        assert_eq!(pick_secret_index(&[RSA], true), Some(0));
        assert_eq!(pick_secret_index(&[ECDSA], false), Some(0));
    }

    fn make_self_signed_pem(cn: &str) -> String {
        use rcgen::{CertificateParams, KeyPair};
        let key = KeyPair::generate().expect("rcgen key generation failed");
        let params = CertificateParams::new(vec![cn.to_string()]).expect("rcgen params failed");
        params.self_signed(&key).expect("rcgen self-sign failed").pem()
    }

    #[test]
    fn ca_bundle_loads_all_pem_blocks() {
        use super::add_ca_bundle_to_store;
        use pingora_core::tls::x509::store::X509StoreBuilder;
        use pingora_core::tls::x509::X509;

        // Realistic shape: a root + two intermediates concatenated into ca.crt.
        let bundle = format!(
            "{}{}{}",
            make_self_signed_pem("ca-root"),
            make_self_signed_pem("ca-intermediate-1"),
            make_self_signed_pem("ca-intermediate-2"),
        );

        // Regression guard: the old `X509::from_pem` path saw only the first
        // block; the helper must load all three.
        assert_eq!(X509::stack_from_pem(bundle.as_bytes()).unwrap().len(), 3);

        let mut builder = X509StoreBuilder::new().expect("store builder");
        let added = add_ca_bundle_to_store(&mut builder, bundle.as_bytes()).expect("add bundle");
        assert_eq!(added, 3, "all three CA certs must be added to the store");
    }

    #[test]
    fn ca_bundle_rejects_empty_or_garbage() {
        use super::add_ca_bundle_to_store;
        use pingora_core::tls::x509::store::X509StoreBuilder;

        let mut builder = X509StoreBuilder::new().expect("store builder");
        assert!(add_ca_bundle_to_store(&mut builder, b"").is_err());
        assert!(add_ca_bundle_to_store(&mut builder, b"not a pem at all").is_err());
    }
}
