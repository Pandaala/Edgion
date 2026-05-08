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
            return;
        }
        if let Some(gateway_tls) = ssl_ctx.matched_gateway_tls.clone() {
            self.apply_gateway_tls_cert(ssl, &gateway_tls, &mut ssl_ctx);
            if ssl_ctx.meta.err_log.is_some() {
                log_ssl(&ssl_ctx.meta);
                return;
            }
            self.finalize_ssl_ctx(ssl, &mut ssl_ctx);
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

            ssl_ctx.meta.handshake_complete_time = Some(ssl_ctx.meta.start_at.elapsed().as_millis() as u64);
            log_ssl(&ssl_ctx.meta);

            return Some(Arc::new(ssl_ctx.meta));
        }

        // Non-BoringSSL fallback: return a minimal TlsConnMeta so downstream
        // consumers (TLS proxy, HTTP proxy) can still read it from the digest.
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
        log_ssl(&meta);
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
    fn finalize_ssl_ctx(&self, ssl: &mut SslRef, ssl_ctx: &mut SslCtx) {
        let rand: u64 = random();
        ssl_ctx.meta.tls_id = Some(format!("{:016x}-{:016x}", ssl_ctx.meta.ts, rand));
        if !store_ssl_ctx(ssl, ssl_ctx) {
            let _ = ssl_ctx
                .meta
                .log
                .get_or_insert_with(LogBuffer::new)
                .push("store ctx failed");
        }
    }

    /// Apply certificate from EdgionTls resource
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

        if let Some(ref client_auth) = edgion_tls.spec.client_auth {
            if let Err(e) = self.configure_mtls(ssl, client_auth, edgion_tls) {
                ssl_ctx.meta.err_log = Some(format!("mtls config: {e}"));
                return;
            }
        }

        if let Some(ref ciphers) = edgion_tls.spec.ciphers {
            if let Err(_e) = self.configure_ciphers(ssl, ciphers) {
                let _ = ssl_ctx
                    .meta
                    .log
                    .get_or_insert_with(LogBuffer::new)
                    .push_err("cipher config fail");
            }
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

        let secret = if let Some(secrets) = &gateway_tls.secrets {
            if let Some(s) = secrets.first() {
                s.clone()
            } else {
                self.get_secret_from_store_or_error(gateway_tls, ssl_ctx)
            }
        } else {
            self.get_secret_from_store_or_error(gateway_tls, ssl_ctx)
        };

        if ssl_ctx.meta.err_log.is_some() {
            return;
        }
        self.apply_secret_to_ssl(ssl, &secret, ssl_ctx);

        // Skip mTLS if server cert setup failed
        if ssl_ctx.meta.err_log.is_some() {
            return;
        }

        // Apply frontend validation mTLS if configured
        if gateway_tls.frontend_validation.is_some() {
            if let Err(e) = self.configure_gateway_frontend_mtls(ssl, gateway_tls) {
                ssl_ctx.meta.err_log = Some(format!("frontend mTLS error: {}", e));
            } else {
                ssl_ctx.meta.is_mtls = true;
            }
        }
    }

    /// Resolve Secret via the global SecretStore by `certificate_refs[0]`.
    fn get_secret_from_store_or_error(
        &self,
        gateway_tls: &GatewayTlsEntry,
        ssl_ctx: &mut SslCtx,
    ) -> k8s_openapi::api::core::v1::Secret {
        let cert_ref = match gateway_tls.certificate_refs.first() {
            Some(r) => r,
            None => {
                ssl_ctx.meta.err_log = Some("no cert refs".to_string());
                return k8s_openapi::api::core::v1::Secret::default();
            }
        };

        let secret_namespace = cert_ref.namespace.as_deref().unwrap_or(&gateway_tls.gateway_namespace);

        match get_secret_by_name(Some(secret_namespace), &cert_ref.name) {
            Some(s) => s,
            None => {
                ssl_ctx.meta.err_log = Some(format!("secret missing: {}/{}", secret_namespace, cert_ref.name));
                k8s_openapi::api::core::v1::Secret::default()
            }
        }
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
    fn configure_gateway_frontend_mtls(
        &self,
        ssl: &mut SslRef,
        gateway_tls: &GatewayTlsEntry,
    ) -> Result<(), Box<PingoraError>> {
        let ca_secrets = match &gateway_tls.resolved_frontend_ca_secrets {
            Some(secrets) if !secrets.is_empty() => secrets,
            _ => return Ok(()),
        };

        let mut store_builder = X509StoreBuilder::new().map_err(|e| {
            PingoraError::explain(ErrorType::InvalidCert, format!("Failed to create X509 store: {}", e))
        })?;

        for secret in ca_secrets {
            if let Some(data) = &secret.data {
                if let Some(ca_bytes) = data.get(CA_CERT) {
                    let ca_pem = std::str::from_utf8(&ca_bytes.0).map_err(|e| {
                        PingoraError::explain(ErrorType::InvalidCert, format!("Invalid CA cert UTF-8: {}", e))
                    })?;
                    let ca_cert = X509::from_pem(ca_pem.as_bytes()).map_err(|e| {
                        PingoraError::explain(ErrorType::InvalidCert, format!("Invalid CA certificate PEM: {}", e))
                    })?;
                    store_builder.add_cert(ca_cert).map_err(|e| {
                        PingoraError::explain(ErrorType::InvalidCert, format!("Failed to add CA cert to store: {}", e))
                    })?;
                }
            }
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

        Ok(())
    }

    /// Configure mTLS (mutual TLS) client certificate verification
    fn configure_mtls(
        &self,
        ssl: &mut SslRef,
        client_auth: &ClientAuthConfig,
        edgion_tls: &Arc<EdgionTls>,
    ) -> Result<(), Box<PingoraError>> {
        let ca_pem = edgion_tls.ca_cert_pem().map_err(|e| {
            PingoraError::explain(
                ErrorType::InvalidCert,
                format!("Failed to get CA cert PEM for mTLS: {}", e),
            )
        })?;

        let ca_cert = X509::from_pem(ca_pem.as_bytes())
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Invalid CA certificate PEM: {}", e)))?;

        let mut store_builder = X509StoreBuilder::new().map_err(|e| {
            PingoraError::explain(ErrorType::InvalidCert, format!("Failed to create X509 store: {}", e))
        })?;

        store_builder.add_cert(ca_cert).map_err(|e| {
            PingoraError::explain(ErrorType::InvalidCert, format!("Failed to add CA cert to store: {}", e))
        })?;

        let store = store_builder.build();

        ssl.set_verify_cert_store(store)
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Failed to set CA store: {}", e)))?;

        let verify_mode = match client_auth.mode {
            ClientAuthMode::Terminate => return Ok(()),
            ClientAuthMode::Mutual => SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT,
            ClientAuthMode::OptionalMutual => SslVerifyMode::PEER,
        };

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
