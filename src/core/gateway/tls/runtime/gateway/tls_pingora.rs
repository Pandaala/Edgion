use crate::core::controller::conf_mgr::sync_runtime::resource_processor::get_secret_by_name;
use crate::core::gateway::observe::logs::LogBuffer;
use crate::core::gateway::observe::logs::ssl_log::log_ssl;
use crate::core::gateway::runtime::matching::{match_gateway_tls_with_port, GatewayTlsEntry};
use crate::core::gateway::tls::runtime::backend::cert_extractor::extract_client_cert_info;
use crate::core::gateway::tls::runtime::backend::set_mtls_verify_callback;
use crate::core::gateway::tls::store::cert_matcher::match_sni_with_port;
use crate::types::constants::secret_keys::tls::{CERT, KEY};
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;
use crate::types::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode, EdgionTls};
use crate::types::{TlsConnMeta, TlsMatchedInfo};
use anyhow::anyhow;
use anyhow::Result;
use pingora_core::listeners::tls::TlsSettings;
use pingora_core::listeners::TlsAccept;
use pingora_core::protocols::tls::TlsRef;
use pingora_core::tls::pkey::PKey;
use pingora_core::tls::ssl::{NameType, SslRef, SslVerifyMode};
use pingora_core::tls::x509::store::X509StoreBuilder;
use pingora_core::tls::x509::X509;
use pingora_core::{Error as PingoraError, ErrorType};
use rand::random;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// SslCtx: per-connection TLS context passed between callbacks via ex_data
// ---------------------------------------------------------------------------

/// Per-connection TLS context. Created in certificate_callback, taken by
/// handshake_complete_callback and finally moved into Pingora's digest.
#[derive(Debug, Clone)]
struct SslCtx {
    meta: TlsConnMeta,
    matched_edgion_tls: Option<Arc<EdgionTls>>,
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

    /// Free callback invoked by BoringSSL when the SSL object is destroyed.
    /// Handles the case where handshake failed before take_ssl_ctx was called.
    unsafe extern "C" fn free_ssl_ctx(
        _parent: *mut c_void,
        ptr: *mut c_void,
        _ad: *mut boring_sys::CRYPTO_EX_DATA,
        _idx: c_int,
        _argl: c_long,
        _argp: *mut c_void,
    ) {
        if !ptr.is_null() {
            let _ = Box::from_raw(ptr as *mut SslCtx);
        }
    }

    /// Store SslCtx into SSL object via ex_data. Returns false and emits ssl.log on failure.
    pub(super) fn store_ssl_ctx(ssl: &mut SslRef, ctx: &SslCtx) -> bool {
        unsafe {
            if !init_ssl_ctx_ex_data_idx() {
                let mut log_meta = ctx.meta.clone();
                let _ = log_meta
                    .log
                    .get_or_insert_with(LogBuffer::new)
                    .push("Failed to initialize SslCtx ex_data index");
                crate::core::gateway::observe::logs::ssl_log::log_ssl(&log_meta);
                return false;
            }
            let idx = SSL_CTX_EX_DATA_IDX.load(Ordering::Acquire);
            let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;

            let ptr = Box::into_raw(Box::new(ctx.clone())) as *mut c_void;
            if boring_sys::SSL_set_ex_data(ssl_ptr, idx, ptr) == 0 {
                let _ = Box::from_raw(ptr as *mut SslCtx);
                let mut log_meta = ctx.meta.clone();
                let _ = log_meta
                    .log
                    .get_or_insert_with(LogBuffer::new)
                    .push("SSL_set_ex_data failed for SslCtx");
                crate::core::gateway::observe::logs::ssl_log::log_ssl(&log_meta);
                return false;
            }
            true
        }
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
            .push("SslCtx ex_data is unavailable without boringssl");
        crate::core::gateway::observe::logs::ssl_log::log_ssl(&log_meta);
        false
    }

    pub(super) fn take_ssl_ctx(_ssl: &TlsRef) -> Option<SslCtx> {
        None
    }
}

use ssl_ctx_ex_data::{store_ssl_ctx, take_ssl_ctx};

// ---------------------------------------------------------------------------
// TlsCallback
// ---------------------------------------------------------------------------

/// TLS callback handler for dynamic certificate loading.
///
/// Supports port-based certificate lookup for Gateway API semantics.
pub struct TlsCallback {
    port: u16,
    edgion_gateway_config: Arc<EdgionGatewayConfig>,
}

#[async_trait::async_trait]
impl TlsAccept for TlsCallback {
    async fn certificate_callback(&self, ssl: &mut TlsRef) {
        let mut ssl_ctx = SslCtx::new(self.port);
        // 1. Resolve SNI — done ONCE per connection
        let sni = if let Some(sni) = ssl.servername(NameType::HOST_NAME) {
            sni.to_string()
        } else if let Some(fallback_sni) = self
            .edgion_gateway_config
            .spec
            .security_protect
            .as_ref()
            .and_then(|sp| sp.fallback_sni.clone())
        {
            let _ = ssl_ctx.meta.log.get_or_insert_with(LogBuffer::new).push("Use Fallback");
            fallback_sni
        } else {
            ssl_ctx.meta.err_log = Some("No sni, No FallBack".to_string());
            log_ssl(&ssl_ctx.meta);
            return;
        };

        ssl_ctx.meta.sni = Some(sni.clone());

        // 2. Match & apply certificate — done ONCE per connection
        if !self.match_and_apply_cert(ssl, &sni, &mut ssl_ctx) {
            log_ssl(&ssl_ctx.meta);
            return;
        }

        // matched, gen tls id
        let rand: u32 = random();
        ssl_ctx.meta.tls_id = Some(format!("{:08x}-{:08x}", ssl_ctx.meta.ts as u32, rand));

        // 3. Store SslCtx for handshake_complete_callback
        if !store_ssl_ctx(ssl, &ssl_ctx) {
            let _ = ssl_ctx.meta.log.get_or_insert_with(LogBuffer::new).push("Failed to store SslCtx");
        }
    }

    async fn handshake_complete_callback(&self, ssl: &TlsRef) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
        if let Some(mut ssl_ctx) = take_ssl_ctx(ssl) {
            // Extract client cert info now that handshake is complete (mTLS verified)
            if let Some(ref edgion_tls) = ssl_ctx.matched_edgion_tls {
                if edgion_tls.is_mtls_enabled() && edgion_tls.should_expose_client_cert() {
                    ssl_ctx.meta.client_cert_info = extract_client_cert_info(ssl);
                }
            }

            ssl_ctx.meta.handshake_complete_time = Some(ssl_ctx.meta.start_at.elapsed().as_millis() as u64);
            log_ssl(&ssl_ctx.meta);

            return Some(Arc::new(ssl_ctx.meta));
        }

        // todo, Do we Need FallBack if no SslCtx?  (non-BoringSSL or store failed)
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
                let _ = buf.push("No SslCtx in handshake_complete_callback");
                buf
            }),
            matched: None,
            err_log: None,
            client_cert_info: None,
            is_mtls: false,
        };
        log_ssl(&meta);
        None
    }
}

impl TlsCallback {
    /// Create a new TlsCallback with port information
    ///
    /// # Parameters
    /// - `port`: The listening port for this TLS service
    /// - `edgion_gateway_config`: Gateway configuration for fallback behavior
    pub fn new(port: u16, edgion_gateway_config: Arc<EdgionGatewayConfig>) -> Self {
        Self {
            port,
            edgion_gateway_config,
        }
    }

    /// Create TLS settings with callback for the specified port
    ///
    /// # Parameters
    /// - `port`: The listening port (used for port-dimension certificate lookup)
    /// - `edgion_gateway_config`: Gateway configuration
    /// - `enable_http2`: Whether to enable HTTP/2 ALPN negotiation
    pub fn new_tls_settings_with_callback(
        port: u16,
        edgion_gateway_config: Arc<EdgionGatewayConfig>,
        enable_http2: bool,
    ) -> Result<TlsSettings> {
        let callback = Box::new(TlsCallback::new(port, edgion_gateway_config));
        let mut settings =
            TlsSettings::with_callbacks(callback).map_err(|e| anyhow!("Failed to create TLS settings: {}", e))?;

        if enable_http2 {
            settings.enable_h2();
        }

        Ok(settings)
    }

    /// Match SNI against certificate stores and apply the matched cert to SSL.
    /// Returns None when no certificate was matched.
    ///
    /// `apply_edgion_tls_cert` / `apply_gateway_tls_cert` handle ssl_log
    /// population internally (cert, mtls, error fields).
    fn match_and_apply_cert(
        &self,
        ssl: &mut SslRef,
        sni: &str,
        ssl_ctx: &mut SslCtx,
    ) -> bool {
        // Layer 1: EdgionTls (port-aware)
        if let Some(edgion_tls) = match_sni_with_port(self.port, sni) {
            ssl_ctx.meta.is_mtls = edgion_tls.spec.client_auth.is_some();
            ssl_ctx.matched_edgion_tls = Some(edgion_tls.clone());
            self.apply_edgion_tls_cert(ssl, &edgion_tls, ssl_ctx);
            return true;
        }

        // Layer 2: Gateway TLS (port-aware)
        if let Some(gw) = match_gateway_tls_with_port(self.port, sni) {
            self.apply_gateway_tls_cert(ssl, &gw, ssl_ctx);
            return true;
        }

        ssl_ctx.meta.err_log = Some("Certificate not found".to_string());
        false
    }

    /// Apply certificate from EdgionTls resource
    fn apply_edgion_tls_cert(&self, ssl: &mut SslRef, edgion_tls: &Arc<EdgionTls>, ssl_ctx: &mut SslCtx) {
        let ns = edgion_tls.metadata.namespace.as_deref().unwrap_or("-");
        let name = edgion_tls.metadata.name.as_deref().unwrap_or("-");
        ssl_ctx.meta.matched = Some(TlsMatchedInfo {
            kind: "EdgionTls".to_string(),
            ns: ns.to_string(),
            name: name.to_string(),
            section: None,
        });

        let cert_pem = match edgion_tls.cert_pem() {
            Ok(pem) => pem,
            Err(e) => {
                ssl_ctx.meta.err_log = Some(format!("Failed to get cert: {}", e));
                return;
            }
        };
        let cert = match X509::from_pem(cert_pem.as_bytes()) {
            Ok(c) => c,
            Err(e) => {
                ssl_ctx.meta.err_log = Some(format!("Invalid cert PEM: {}", e));
                return;
            }
        };
        if let Err(e) = pingora_core::tls::ext::ssl_use_certificate(ssl, &cert) {
            ssl_ctx.meta.err_log = Some(format!("Failed to use cert: {}", e));
            return;
        }

        let key_pem = match edgion_tls.key_pem() {
            Ok(pem) => pem,
            Err(e) => {
                ssl_ctx.meta.err_log = Some(format!("Failed to get key: {}", e));
                return;
            }
        };
        let key = match PKey::private_key_from_pem(key_pem.as_bytes()) {
            Ok(k) => k,
            Err(e) => {
                ssl_ctx.meta.err_log = Some(format!("Invalid key PEM: {}", e));
                return;
            }
        };
        if let Err(e) = pingora_core::tls::ext::ssl_use_private_key(ssl, &key) {
            ssl_ctx.meta.err_log = Some(format!("Failed to use key: {}", e));
            return;
        }

        if let Some(ref client_auth) = edgion_tls.spec.client_auth {
            if let Err(e) = self.configure_mtls(ssl, client_auth, edgion_tls) {
                ssl_ctx.meta.err_log = Some(format!("mTLS config failed: {}", e));
                return;
            }
        }

        if let Some(min_version) = edgion_tls.spec.min_tls_version {
            if let Err(e) = self.configure_min_tls_version(ssl, min_version) {
                ssl_ctx.meta.err_log = Some(format!("TLS version config failed: {}", e));
                return;
            }
        }

        if let Some(ref ciphers) = edgion_tls.spec.ciphers {
            if let Err(e) = self.configure_ciphers(ssl, ciphers) {
                let _ = ssl_ctx
                    .meta
                    .log
                    .get_or_insert_with(LogBuffer::new)
                    .push(&format!("[err] Cipher config failed: {}", e));
            }
        }
    }

    /// Apply certificate from Gateway Listener TLS configuration (from Secret)
    fn apply_gateway_tls_cert(&self, ssl: &mut SslRef, gateway_tls: &GatewayTlsEntry, ssl_ctx: &mut SslCtx) {
        ssl_ctx.meta.matched = Some(TlsMatchedInfo {
            kind: "Gateway".to_string(),
            ns: gateway_tls.gateway_namespace.clone(),
            name: gateway_tls.gateway_name.clone(),
            section: Some(gateway_tls.listener_name.clone()),
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

        self.apply_secret_to_ssl(ssl, &secret, ssl_ctx);
    }

    /// Helper: Get Secret from global SecretStore (fallback for legacy behavior)
    fn get_secret_from_store_or_error(
        &self,
        gateway_tls: &GatewayTlsEntry,
        ssl_ctx: &mut SslCtx,
    ) -> k8s_openapi::api::core::v1::Secret {
        let cert_ref = match gateway_tls.certificate_refs.first() {
            Some(r) => r,
            None => {
                ssl_ctx.meta.err_log = Some("No certificate refs in Gateway TLS config".to_string());
                return k8s_openapi::api::core::v1::Secret::default();
            }
        };

        let secret_namespace = cert_ref.namespace.as_deref().unwrap_or(&gateway_tls.gateway_namespace);

        match get_secret_by_name(Some(secret_namespace), &cert_ref.name) {
            Some(s) => s,
            None => {
                ssl_ctx.meta.err_log = Some(format!("Secret not found: {}/{}", secret_namespace, cert_ref.name));
                k8s_openapi::api::core::v1::Secret::default()
            }
        }
    }

    /// Helper: Extract cert/key from Secret and apply to SSL
    fn apply_secret_to_ssl(
        &self,
        ssl: &mut SslRef,
        secret: &k8s_openapi::api::core::v1::Secret,
        ssl_ctx: &mut SslCtx,
    ) {
        let data = match &secret.data {
            Some(d) => d,
            None => {
                ssl_ctx.meta.err_log = Some("Secret has no data".to_string());
                return;
            }
        };

        let cert_pem = match data.get(CERT) {
            Some(bytes) => match String::from_utf8(bytes.0.clone()) {
                Ok(s) => s,
                Err(e) => {
                    ssl_ctx.meta.err_log = Some(format!("Invalid {} encoding: {}", CERT, e));
                    return;
                }
            },
            None => {
                ssl_ctx.meta.err_log = Some(format!("Secret missing {}", CERT));
                return;
            }
        };

        let key_pem = match data.get(KEY) {
            Some(bytes) => match String::from_utf8(bytes.0.clone()) {
                Ok(s) => s,
                Err(e) => {
                    ssl_ctx.meta.err_log = Some(format!("Invalid {} encoding: {}", KEY, e));
                    return;
                }
            },
            None => {
                ssl_ctx.meta.err_log = Some(format!("Secret missing {}", KEY));
                return;
            }
        };

        let cert = match X509::from_pem(cert_pem.as_bytes()) {
            Ok(c) => c,
            Err(e) => {
                ssl_ctx.meta.err_log = Some(format!("Invalid cert PEM from Secret: {}", e));
                return;
            }
        };
        if let Err(e) = pingora_core::tls::ext::ssl_use_certificate(ssl, &cert) {
            ssl_ctx.meta.err_log = Some(format!("Failed to use cert: {}", e));
            return;
        }

        let key = match PKey::private_key_from_pem(key_pem.as_bytes()) {
            Ok(k) => k,
            Err(e) => {
                ssl_ctx.meta.err_log = Some(format!("Invalid key PEM from Secret: {}", e));
                return;
            }
        };
        if let Err(e) = pingora_core::tls::ext::ssl_use_private_key(ssl, &key) {
            ssl_ctx.meta.err_log = Some(format!("Failed to use key: {}", e));
            return;
        }

        if let Some(name) = &secret.metadata.name {
            tracing::debug!(
                secret_name = %name,
                "Applied certificate from Secret"
            );
        }
    }

    /// Configure mTLS (mutual TLS) client certificate verification
    fn configure_mtls(
        &self,
        ssl: &mut SslRef,
        client_auth: &ClientAuthConfig,
        edgion_tls: &Arc<EdgionTls>,
    ) -> Result<(), Box<PingoraError>> {
        tracing::debug!("Configuring mTLS with mode: {:?}", client_auth.mode);

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
            ClientAuthMode::Terminate => {
                tracing::debug!("mTLS mode: Terminate (single-way TLS)");
                return Ok(());
            }
            ClientAuthMode::Mutual => {
                tracing::debug!("mTLS mode: Mutual (client cert required)");
                SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT
            }
            ClientAuthMode::OptionalMutual => {
                tracing::debug!("mTLS mode: OptionalMutual (client cert optional)");
                SslVerifyMode::PEER
            }
        };

        // SAFETY: verify_depth is validated to be in range 1-9 by cert_validator
        // u8 (1-9) can always be safely converted to u32
        #[cfg(feature = "boringssl")]
        {
            ssl.set_verify_depth(client_auth.verify_depth as u32);
            tracing::debug!("Set mTLS verification depth: {}", client_auth.verify_depth);
        }
        #[cfg(not(feature = "boringssl"))]
        {
            tracing::debug!(
                "Verify depth configuration: {} (using backend default, explicit setting requires boringssl)",
                client_auth.verify_depth
            );
        }

        if client_auth.allowed_sans.is_some() || client_auth.allowed_cns.is_some() {
            tracing::debug!("Setting custom verify callback for SAN/CN whitelist");

            if let Err(e) = set_mtls_verify_callback(ssl, verify_mode, edgion_tls) {
                tracing::error!(
                    "Failed to set mTLS verify callback: {}. \
                    Make sure you're using a compatible TLS backend (BoringSSL).",
                    e
                );
                return Err(PingoraError::explain(
                    ErrorType::InternalError,
                    format!("Failed to set verify callback: {}", e),
                ));
            }

            tracing::info!("Custom verify callback configured for SAN/CN whitelist");
        } else {
            ssl.set_verify(verify_mode);
            tracing::debug!("Set mTLS verify mode (no whitelist): {:?}", verify_mode);
        }

        tracing::info!(
            "mTLS configured successfully for SNI with mode: {:?}, verify_depth: {}",
            client_auth.mode,
            client_auth.verify_depth
        );

        Ok(())
    }

    /// Configure minimum TLS version (similar to Cloudflare's Minimum TLS Version)
    fn configure_min_tls_version(
        &self,
        ssl: &mut SslRef,
        min_version: crate::types::resources::edgion_tls::TlsVersion,
    ) -> Result<(), Box<PingoraError>> {
        use crate::types::resources::edgion_tls::TlsVersion;
        use pingora_core::tls::ssl::SslVersion;

        if matches!(min_version, TlsVersion::Tls10 | TlsVersion::Tls11) {
            tracing::warn!(
                min_version = ?min_version,
                "TLS 1.0/1.1 are deprecated and have known vulnerabilities. Consider using TLS 1.2+"
            );
        }

        let ssl_version = match min_version {
            TlsVersion::Tls10 => SslVersion::TLS1,
            TlsVersion::Tls11 => SslVersion::TLS1_1,
            TlsVersion::Tls12 => SslVersion::TLS1_2,
            TlsVersion::Tls13 => SslVersion::TLS1_3,
        };

        ssl.set_min_proto_version(Some(ssl_version)).map_err(|e| {
            PingoraError::explain(
                ErrorType::InternalError,
                format!("Failed to set min TLS version: {}", e),
            )
        })?;

        tracing::debug!(min_version = ?min_version, "Configured minimum TLS version");
        Ok(())
    }

    /// Configure cipher list (similar to Nginx ssl_ciphers directive)
    ///
    /// Takes a list of cipher names in OpenSSL format and applies them via BoringSSL FFI.
    /// If configuration fails, logs a warning and continues with default ciphers.
    ///
    /// Note: TLS 1.3 ciphers are hardcoded in BoringSSL and cannot be configured.
    fn configure_ciphers(&self, ssl: &mut SslRef, ciphers: &[String]) -> Result<(), Box<PingoraError>> {
        if ciphers.is_empty() {
            return Ok(());
        }

        let cipher_list = ciphers.join(":");

        #[cfg(feature = "boringssl")]
        {
            use std::ffi::CString;

            let cipher_cstr = match CString::new(cipher_list.as_str()) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Invalid cipher list (contains null byte), using default ciphers"
                    );
                    return Ok(());
                }
            };

            // SAFETY: SslRef -> SSL* conversion for FFI call
            // ssl is valid during this function, cipher_cstr lifetime extends past FFI call
            unsafe {
                let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;
                let ret = boring_sys::SSL_set_strict_cipher_list(ssl_ptr, cipher_cstr.as_ptr());

                if ret != 1 {
                    tracing::warn!(
                        cipher_list = %cipher_list,
                        "Failed to set cipher list, using default ciphers. \
                         Check if cipher names are valid for BoringSSL."
                    );
                    return Ok(());
                }
            }

            tracing::debug!(cipher_list = %cipher_list, "Configured cipher list");
        }

        #[cfg(not(feature = "boringssl"))]
        {
            tracing::warn!(
                cipher_list = %cipher_list,
                "Cipher configuration requires BoringSSL backend"
            );
            let _ = ssl;
        }

        Ok(())
    }
}
