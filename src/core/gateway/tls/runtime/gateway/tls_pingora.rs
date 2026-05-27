use crate::core::controller::conf_mgr::sync_runtime::resource_processor::get_secret_by_name;
use crate::core::gateway::observe::logs::ssl_log::log_ssl;
use crate::core::gateway::observe::logs::LogBuffer;
use crate::core::gateway::runtime::matching::{match_gateway_tls, GatewayTlsEntry};
use crate::core::gateway::tls::runtime::backend::cert_extractor::extract_client_cert_info;
use crate::core::gateway::tls::runtime::backend::set_mtls_verify_callback;
use crate::core::gateway::tls::store::cert_matcher::match_sni;
use crate::types::constants::secret_keys::tls::{CA_CERT, CERT, KEY};
use crate::types::listener_key::ListenerKey;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;
use crate::types::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode, EdgionTls};
use crate::types::resources::gateway::FrontendValidationMode;
use crate::types::{MatchedInfo, ResourceMeta, Rsc, TlsConnMeta};
use anyhow::anyhow;
use anyhow::Result;
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
#[derive(Debug, Clone)]
struct SslCtx {
    meta: TlsConnMeta,
    matched_edgion_tls: Option<Arc<EdgionTls>>,
    matched_gateway_tls: Option<GatewayTlsEntry>,
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
        }
    }
}

// ---------------------------------------------------------------------------
// BoringSSL ex_data helpers
// ---------------------------------------------------------------------------

#[cfg(feature = "boringssl")]
mod ssl_ctx_ex_data {
    use super::SslCtx;
    use crate::core::gateway::observe::logs::LogBuffer;
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
        // catch_unwind: any panic from log_ssl crossing the FFI boundary is UB.
        let _ = std::panic::catch_unwind(|| {
            let ssl_ctx = Box::from_raw(ptr as *mut SslCtx);
            if ssl_ctx.meta.tls_id.is_none() {
                let mut meta = ssl_ctx.meta.clone();
                // Preserve a more specific upstream err_log (e.g. "min ver: …")
                // if select_cb already set one.
                if meta.err_log.is_none() {
                    meta.err_log = Some("early abort".to_string());
                }
                crate::core::gateway::observe::logs::ssl_log::log_ssl(&meta);
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
                crate::core::gateway::observe::logs::ssl_log::log_ssl(&log_meta);
                return false;
            }
            let idx = SSL_CTX_EX_DATA_IDX.load(Ordering::Acquire);
            let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;

            // Free any previously stored SslCtx in this slot. BoringSSL's
            // SSL_set_ex_data does NOT invoke the registered free callback
            // when overwriting — the free callback only fires on SSL
            // destruction. Without this, calling store_ssl_ctx twice on the
            // same SSL leaks the first Box<SslCtx>.
            let prev = boring_sys::SSL_get_ex_data(ssl_ptr, idx);
            if !prev.is_null() {
                let _ = Box::from_raw(prev as *mut SslCtx);
            }

            let ptr = Box::into_raw(Box::new(ctx.clone())) as *mut c_void;
            if boring_sys::SSL_set_ex_data(ssl_ptr, idx, ptr) == 0 {
                let _ = Box::from_raw(ptr as *mut SslCtx);
                let mut log_meta = ctx.meta.clone();
                let _ = log_meta
                    .log
                    .get_or_insert_with(LogBuffer::new)
                    .push("exdata store failed");
                crate::core::gateway::observe::logs::ssl_log::log_ssl(&log_meta);
                return false;
            }
            true
        }
    }

    /// Peek at TlsConnMeta in SSL ex_data without taking ownership.
    /// Used by mTLS verify callback to write ssl.log on failure.
    ///
    /// # Safety
    /// `ssl_ptr` must be a valid SSL pointer during the call.
    pub(super) unsafe fn peek_tls_conn_meta(ssl_ptr: *const boring_sys::SSL) -> Option<crate::types::ctx::TlsConnMeta> {
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
            let ctx = Box::from_raw(ptr as *mut SslCtx);
            boring_sys::SSL_set_ex_data(ssl_ptr as *mut boring_sys::SSL, idx, std::ptr::null_mut());
            Some(*ctx)
        }
    }
}

#[cfg(not(feature = "boringssl"))]
mod ssl_ctx_ex_data {
    use super::SslCtx;
    use crate::core::gateway::observe::logs::LogBuffer;
    use pingora_core::protocols::tls::TlsRef;
    use pingora_core::tls::ssl::SslRef;

    pub(super) fn store_ssl_ctx(_ssl: &mut SslRef, ctx: &SslCtx) -> bool {
        let mut log_meta = ctx.meta.clone();
        let _ = log_meta
            .log
            .get_or_insert_with(LogBuffer::new)
            .push("no boringssl exdata");
        crate::core::gateway::observe::logs::ssl_log::log_ssl(&log_meta);
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
pub(crate) unsafe fn peek_tls_conn_meta(ssl_ptr: *const boring_sys::SSL) -> Option<crate::types::ctx::TlsConnMeta> {
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
        // and now also fail-closes on store_ssl_ctx failure. So if cert_cb
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
        // * BoringSSL builds: the only realistic paths into this branch are
        //   store_ssl_ctx failures in select_cb / finalize_ssl_ctx, both of
        //   which already emitted ssl.log with a more complete meta
        //   (tls_id / sni / matched populated). Logging again here would
        //   double-count the connection and the second entry would lack
        //   tls_id / sni / matched, misleading audit consumers.
        // * Non-BoringSSL builds: store_ssl_ctx in select_cb always fails and
        //   returns SelectCertError::ERROR, so cert_cb / this callback are not
        //   reached on that path.
        // The remaining theoretical case (BoringSSL state-machine glitch zeroing
        // ex_data between callbacks) is unreachable-by-design; we accept losing
        // a fallback log entry there in exchange for not double-counting the
        // rare allocation-failure case.
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
fn map_tls_version(v: crate::types::resources::edgion_tls::TlsVersion) -> pingora_core::tls::ssl::SslVersion {
    use crate::types::resources::edgion_tls::TlsVersion;
    use pingora_core::tls::ssl::SslVersion;
    match v {
        TlsVersion::Tls10 => SslVersion::TLS1,
        TlsVersion::Tls11 => SslVersion::TLS1_1,
        TlsVersion::Tls12 => SslVersion::TLS1_2,
        TlsVersion::Tls13 => SslVersion::TLS1_3,
    }
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

                // ── 2. Layer 1: EdgionTls ────────────────────────────────
                if let Some(et) = match_sni(&lk, &sni) {
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
                if let Some(gw) = match_gateway_tls(&lk, &sni) {
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
        ssl_ctx.meta.matched = Some(MatchedInfo {
            rsc: Rsc {
                kind: "EdgionTls".to_string(),
                ns: ns.to_string(),
                name: name.to_string(),
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
        if let Some(ref ciphers) = edgion_tls.spec.ciphers {
            if let Err(e) = self.configure_ciphers(ssl, ciphers) {
                ssl_ctx.meta.err_log = Some(format!("cipher config: {e}"));
                return;
            }
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
                    .get_or_insert_with(crate::core::gateway::observe::logs::LogBuffer::new)
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

    /// Apply certificate from Gateway Listener TLS configuration (from Secret)
    fn apply_gateway_tls_cert(&self, ssl: &mut SslRef, gateway_tls: &GatewayTlsEntry, ssl_ctx: &mut SslCtx) {
        ssl_ctx.meta.matched = Some(MatchedInfo {
            rsc: Rsc {
                kind: "Gateway".to_string(),
                ns: gateway_tls.gateway_namespace.clone(),
                name: gateway_tls.gateway_name.clone(),
            },
            section: Some(gateway_tls.listener_name.clone()),
            sv: 0,
        });

        // Collect every cert/key Secret configured for this listener: prefer the
        // controller-resolved inline secrets, otherwise resolve every
        // certificateRef from the global store.
        let secrets = match &gateway_tls.secrets {
            Some(s) if !s.is_empty() => s.clone(),
            _ => self.resolve_secrets_from_store_or_error(gateway_tls, ssl_ctx),
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
        let secret = match self.select_gateway_secret(ssl, &secrets) {
            Some(s) => s.clone(),
            None => {
                ssl_ctx.meta.err_log = Some("no usable cert".to_string());
                return;
            }
        };
        self.apply_secret_to_ssl(ssl, &secret, ssl_ctx);
        if ssl_ctx.meta.err_log.is_some() {
            return;
        }

        // Apply frontend validation mTLS if configured. The callee reports
        // whether it actually installed a verify mode: a frontendValidation with
        // empty/None CA refs reaches here (the controller does not fence it) and
        // returns Ok(false) without requesting any client cert, so flagging
        // is_mtls=true there would mislead tls.log / tls_route consumers (the
        // same reasoning as the EdgionTls path above, fixed in tls-F001).
        if gateway_tls.frontend_validation.is_some() {
            match self.configure_gateway_frontend_mtls(ssl, gateway_tls) {
                Err(e) => ssl_ctx.meta.err_log = Some(format!("frontend mTLS error: {}", e)),
                Ok(verify_installed) => ssl_ctx.meta.is_mtls = verify_installed,
            }
        }
    }

    /// Resolve every `certificate_refs` entry via the global SecretStore.
    ///
    /// Fallback path used when the controller has not inlined the resolved
    /// secrets. Returns all resolved secrets; a single missing ref fails the
    /// whole listener (fail-closed) by setting `err_log`.
    fn resolve_secrets_from_store_or_error(
        &self,
        gateway_tls: &GatewayTlsEntry,
        ssl_ctx: &mut SslCtx,
    ) -> Vec<k8s_openapi::api::core::v1::Secret> {
        if gateway_tls.certificate_refs.is_empty() {
            ssl_ctx.meta.err_log = Some("no cert refs".to_string());
            return Vec::new();
        }

        let mut secrets = Vec::with_capacity(gateway_tls.certificate_refs.len());
        for cert_ref in &gateway_tls.certificate_refs {
            let secret_namespace = cert_ref.namespace.as_deref().unwrap_or(&gateway_tls.gateway_namespace);
            match get_secret_by_name(Some(secret_namespace), &cert_ref.name) {
                Some(s) => secrets.push(s),
                None => {
                    ssl_ctx.meta.err_log = Some(format!("secret missing: {}/{}", secret_namespace, cert_ref.name));
                    return Vec::new();
                }
            }
        }
        secrets
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

    /// Best-effort cert key-type probe: `Some(true)` for ECDSA, `Some(false)`
    /// for any other key type (RSA), `None` when the cert cannot be parsed (the
    /// caller then falls through in its selection).
    #[cfg(feature = "boringssl")]
    fn secret_is_ecdsa(secret: &k8s_openapi::api::core::v1::Secret) -> Option<bool> {
        let cert_bytes = secret.data.as_ref()?.get(CERT)?;
        let cert = X509::from_pem(&cert_bytes.0).ok()?;
        let pkey = cert.public_key().ok()?;
        Some(pkey.id() == pingora_core::tls::pkey::Id::EC)
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
    fn select_gateway_secret<'a>(
        &self,
        ssl: &SslRef,
        secrets: &'a [k8s_openapi::api::core::v1::Secret],
    ) -> Option<&'a k8s_openapi::api::core::v1::Secret> {
        if secrets.len() <= 1 {
            return secrets.first();
        }
        let prefer_ecdsa = self.negotiated_is_tls13(ssl) && self.client_supports_ecdsa(ssl);
        let kinds: Vec<Option<bool>> = secrets.iter().map(Self::secret_is_ecdsa).collect();
        pick_secret_index(&kinds, prefer_ecdsa).map(|i| &secrets[i])
    }

    /// Non-BoringSSL fallback: no per-key-type selection primitive is wired up,
    /// so serve the first configured cert (unchanged legacy behavior).
    #[cfg(not(feature = "boringssl"))]
    fn select_gateway_secret<'a>(
        &self,
        _ssl: &SslRef,
        secrets: &'a [k8s_openapi::api::core::v1::Secret],
    ) -> Option<&'a k8s_openapi::api::core::v1::Secret> {
        secrets.first()
    }

    /// Helper: Extract cert/key from Secret and apply to SSL
    fn apply_secret_to_ssl(&self, ssl: &mut SslRef, secret: &k8s_openapi::api::core::v1::Secret, ssl_ctx: &mut SslCtx) {
        let data = match &secret.data {
            Some(d) => d,
            None => {
                ssl_ctx.meta.err_log = Some("secret empty".to_string());
                return;
            }
        };

        let cert_pem = match data.get(CERT) {
            Some(bytes) => match String::from_utf8(bytes.0.clone()) {
                Ok(s) => s,
                Err(e) => {
                    ssl_ctx.meta.err_log = Some(format!("bad {CERT}: {e}"));
                    return;
                }
            },
            None => {
                ssl_ctx.meta.err_log = Some(format!("no {CERT}"));
                return;
            }
        };

        let key_pem = match data.get(KEY) {
            Some(bytes) => match String::from_utf8(bytes.0.clone()) {
                Ok(s) => s,
                Err(e) => {
                    ssl_ctx.meta.err_log = Some(format!("bad {KEY}: {e}"));
                    return;
                }
            },
            None => {
                ssl_ctx.meta.err_log = Some(format!("no {KEY}"));
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
        // Runtime expiry recheck — cert may have expired between controller
        // rebuilds. Mirrors the EdgionTls path in apply_edgion_tls_cert so that
        // GatewayTls-sourced certs emit the same cert_expired / cert_not_yet_valid
        // audit signal (RFC 5280 inclusive bounds). The `&now` of the EdgionTls
        // block is dropped here on purpose — clippy::op_ref flags it, and
        // `Asn1Time`/`Asn1TimeRef` cross-compare unchanged either way.
        #[cfg(feature = "boringssl")]
        match boring::asn1::Asn1Time::days_from_now(0) {
            Ok(now) => {
                if cert.not_after() < now {
                    ssl_ctx.meta.err_log = Some(format!("cert_expired: not_after={}", cert.not_after()));
                    return;
                }
                if cert.not_before() > now {
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
                    .get_or_insert_with(crate::core::gateway::observe::logs::LogBuffer::new)
                    .push_err("cert_expiry_check_failed");
            }
        }
        if let Err(e) = pingora_core::tls::ext::ssl_use_certificate(ssl, &cert) {
            ssl_ctx.meta.err_log = Some(format!("set cert: {e}"));
            return;
        }

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
        let ca_secrets = match &gateway_tls.resolved_frontend_ca_secrets {
            Some(secrets) if !secrets.is_empty() => secrets,
            _ => return Ok(false),
        };

        let mut store_builder = X509StoreBuilder::new().map_err(|e| {
            PingoraError::explain(ErrorType::InvalidCert, format!("Failed to create X509 store: {}", e))
        })?;

        let mut certs_added: usize = 0;
        for secret in ca_secrets {
            if let Some(data) = &secret.data {
                if let Some(ca_bytes) = data.get(CA_CERT) {
                    certs_added += add_ca_bundle_to_store(&mut store_builder, &ca_bytes.0)?;
                }
            }
        }

        // Symmetric with the fail-closed branch inside `add_ca_bundle_to_store`
        // for empty/garbage PEM: if every referenced Secret lacked the `ca.crt`
        // key, the loop body never ran and we are about to build an empty trust
        // store. Returning Err here lets the caller stamp `err_log` instead of
        // silently installing a barrier that rejects every client.
        if certs_added == 0 {
            return Err(PingoraError::explain(
                ErrorType::InvalidCert,
                format!(
                    "frontendValidation references {} CA Secret(s), but none contained the `{}` key",
                    ca_secrets.len(),
                    CA_CERT
                ),
            ));
        }

        let store = store_builder.build();
        ssl.set_verify_cert_store(store)
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Failed to set CA store: {}", e)))?;

        // Determine verify mode based on frontend validation mode
        let mode = gateway_tls
            .frontend_validation
            .as_ref()
            .and_then(|fv| fv.mode.as_ref())
            .cloned()
            .unwrap_or_default();

        if matches!(mode, FrontendValidationMode::AllowInsecureFallback) {
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

    /// Configure mTLS (mutual TLS) client certificate verification
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

        // SAFETY: verify_depth is validated to be in range 1-9 by cert_validator
        // u8 (1-9) can always be safely converted to u32
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
    let stack = X509::stack_from_pem(ca_pem)
        .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Invalid CA certificate PEM: {}", e)))?;
    if stack.is_empty() {
        return Err(PingoraError::explain(
            ErrorType::InvalidCert,
            "CA certificate PEM contained no certificates".to_string(),
        ));
    }
    let count = stack.len();
    for cert in stack {
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
#[cfg(feature = "boringssl")]
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
    use super::pick_secret_index;

    const RSA: Option<bool> = Some(false);
    const ECDSA: Option<bool> = Some(true);
    const UNKNOWN: Option<bool> = None;

    #[test]
    fn empty_list_returns_none() {
        assert_eq!(pick_secret_index(&[], true), None);
        assert_eq!(pick_secret_index(&[], false), None);
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
