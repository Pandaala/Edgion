use crate::core::controller::conf_mgr::sync_runtime::resource_processor::get_secret_by_name;
use crate::core::gateway::observe::logs::ssl_log::{log_ssl, SslLogEntry};
use crate::core::gateway::runtime::matching::{match_gateway_tls, match_gateway_tls_with_port, GatewayTlsEntry};
use crate::core::gateway::tls::runtime::backend::cert_extractor::extract_client_cert_info;
use crate::core::gateway::tls::runtime::backend::set_mtls_verify_callback;
use crate::core::gateway::tls::store::cert_matcher::match_sni_with_port;
use crate::types::constants::secret_keys::tls::{CERT, KEY};
use crate::types::ctx::ClientCertInfo;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;
use crate::types::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode, EdgionTls};
use crate::types::{gen_tls_id, CertSource, TlsConnMeta};
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
use std::sync::Arc;

// ---------------------------------------------------------------------------
// SslCtx: per-connection TLS context passed between callbacks via ex_data
// ---------------------------------------------------------------------------

/// Per-connection TLS context. Created in certificate_callback, taken by
/// handshake_complete_callback, then moved into TlsConnMeta.
///
/// Intentionally does NOT derive Clone — owned by one callback at a time.
#[derive(Debug)]
struct SslCtx {
    tls_id: String,
    sni: Option<String>,
    cert_source: CertSource,
    is_mtls: bool,
    matched_edgion_tls: Option<Arc<EdgionTls>>,
    client_cert_info: Option<ClientCertInfo>,
}

// ---------------------------------------------------------------------------
// BoringSSL ex_data helpers
// ---------------------------------------------------------------------------

#[cfg(feature = "boringssl")]
mod ssl_ctx_ex_data {
    use super::SslCtx;
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

    /// Store SslCtx into SSL object via ex_data. Takes ownership via Box.
    pub(super) fn store_ssl_ctx(ssl: &mut SslRef, ctx: SslCtx) -> std::result::Result<(), String> {
        unsafe {
            if !init_ssl_ctx_ex_data_idx() {
                return Err("Failed to initialize SslCtx ex_data index".to_string());
            }
            let idx = SSL_CTX_EX_DATA_IDX.load(Ordering::Acquire);
            let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;

            let ptr = Box::into_raw(Box::new(ctx)) as *mut c_void;
            if boring_sys::SSL_set_ex_data(ssl_ptr, idx, ptr) == 0 {
                let _ = Box::from_raw(ptr as *mut SslCtx);
                return Err("SSL_set_ex_data failed for SslCtx".to_string());
            }
            Ok(())
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
    use pingora_core::protocols::tls::TlsRef;
    use pingora_core::tls::ssl::SslRef;

    pub(super) fn store_ssl_ctx(_ssl: &mut SslRef, _ctx: SslCtx) -> std::result::Result<(), String> {
        Ok(())
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
        let tls_id = gen_tls_id();
        let mut entry = SslLogEntry::new();
        entry.tls_id(&tls_id);

        // 1. Resolve SNI — done ONCE per connection
        let sni = match self.resolve_sni(ssl) {
            Ok(s) => s,
            Err(e) => {
                entry.error(&e);
                log_ssl(&entry);
                let _ = store_ssl_ctx(
                    ssl,
                    SslCtx {
                        tls_id: tls_id.clone(),
                        sni: None,
                        cert_source: CertSource::NotFound,
                        is_mtls: false,
                        matched_edgion_tls: None,
                        client_cert_info: None,
                    },
                );
                return;
            }
        };
        entry.sni(&sni);

        // 2. Match & apply certificate — done ONCE per connection
        let (cert_source, matched_edgion_tls, is_mtls) = self.match_and_apply_cert(ssl, &sni, &mut entry);

        // 3. Write ssl.log immediately (no re-matching needed)
        log_ssl(&entry);

        // 4. Store SslCtx for handshake_complete_callback
        let ssl_ctx = SslCtx {
            tls_id,
            sni: Some(sni),
            cert_source,
            is_mtls,
            matched_edgion_tls,
            client_cert_info: None,
        };
        if let Err(e) = store_ssl_ctx(ssl, ssl_ctx) {
            tracing::error!("Failed to store SslCtx: {}", e);
        }
    }

    async fn handshake_complete_callback(&self, ssl: &TlsRef) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
        if let Some(mut ssl_ctx) = take_ssl_ctx(ssl) {
            // Extract client cert info now that handshake is complete (mTLS verified)
            if let Some(ref edgion_tls) = ssl_ctx.matched_edgion_tls {
                if edgion_tls.is_mtls_enabled() && edgion_tls.should_expose_client_cert() {
                    ssl_ctx.client_cert_info = extract_client_cert_info(ssl);
                }
            }

            return Some(Arc::new(TlsConnMeta {
                tls_id: ssl_ctx.tls_id,
                sni: ssl_ctx.sni,
                client_cert_info: ssl_ctx.client_cert_info,
                cert_source: ssl_ctx.cert_source,
                is_mtls: ssl_ctx.is_mtls,
            }));
        }

        // Fallback: no SslCtx (non-BoringSSL or store failed)
        let tls_id = gen_tls_id();
        let sni = ssl.servername(NameType::HOST_NAME).map(|s| s.to_string());
        Some(Arc::new(TlsConnMeta {
            tls_id,
            sni,
            client_cert_info: None,
            cert_source: CertSource::NotFound,
            is_mtls: false,
        }))
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

    /// Resolve SNI from SSL object with fallback support.
    /// Called exactly once per connection in certificate_callback.
    fn resolve_sni(&self, ssl: &TlsRef) -> std::result::Result<String, String> {
        if let Some(s) = ssl.servername(NameType::HOST_NAME) {
            return Ok(s.to_string());
        }
        self.edgion_gateway_config
            .spec
            .security_protect
            .as_ref()
            .and_then(|sp| sp.fallback_sni.clone())
            .ok_or_else(|| "No SNI provided and no fallback configured".to_string())
    }

    /// Match SNI against certificate stores and apply the matched cert to SSL.
    /// Returns (CertSource, Option<Arc<EdgionTls>>, is_mtls).
    ///
    /// `apply_edgion_tls_cert` / `apply_gateway_tls_cert` handle log_entry
    /// population internally (cert, mtls, error fields).
    fn match_and_apply_cert(
        &self,
        ssl: &mut SslRef,
        sni: &str,
        entry: &mut SslLogEntry,
    ) -> (CertSource, Option<Arc<EdgionTls>>, bool) {
        // Layer 1: EdgionTls (port-aware)
        if let Ok(edgion_tls) = match_sni_with_port(self.port, sni) {
            let ns = edgion_tls.metadata.namespace.as_deref().unwrap_or("-").to_string();
            let name = edgion_tls.metadata.name.as_deref().unwrap_or("-").to_string();
            let is_mtls = edgion_tls.spec.client_auth.is_some();
            self.apply_edgion_tls_cert(ssl, &edgion_tls, entry);
            return (CertSource::EdgionTls { namespace: ns, name }, Some(edgion_tls), is_mtls);
        }

        // Layer 2: Gateway TLS (port-aware)
        if let Ok(gw) = match_gateway_tls_with_port(self.port, sni) {
            let source = CertSource::GatewayTls {
                gateway_namespace: gw.gateway_namespace.clone(),
                gateway_name: gw.gateway_name.clone(),
                listener_name: gw.listener_name.clone(),
            };
            self.apply_gateway_tls_cert(ssl, &gw, entry);
            return (source, None, false);
        }

        // Layer 2b: Gateway TLS (port-independent fallback)
        if let Ok(gw) = match_gateway_tls(sni) {
            tracing::debug!(
                port = self.port,
                sni = %sni,
                "Certificate found via port-independent fallback"
            );
            let source = CertSource::GatewayTls {
                gateway_namespace: gw.gateway_namespace.clone(),
                gateway_name: gw.gateway_name.clone(),
                listener_name: gw.listener_name.clone(),
            };
            self.apply_gateway_tls_cert(ssl, &gw, entry);
            return (source, None, false);
        }

        entry.error(format!("Certificate not found for port={}, SNI={}", self.port, sni));
        (CertSource::NotFound, None, false)
    }

    /// Apply certificate from EdgionTls resource
    fn apply_edgion_tls_cert(&self, ssl: &mut SslRef, edgion_tls: &Arc<EdgionTls>, entry: &mut SslLogEntry) {
        let ns = edgion_tls.metadata.namespace.as_deref().unwrap_or("-");
        let name = edgion_tls.metadata.name.as_deref().unwrap_or("-");
        entry.cert(format!("EdgionTls:{}/{}", ns, name));
        entry.mtls(edgion_tls.spec.client_auth.is_some());

        let cert_pem = match edgion_tls.cert_pem() {
            Ok(pem) => pem,
            Err(e) => {
                entry.error(format!("Failed to get cert: {}", e));
                return;
            }
        };
        let cert = match X509::from_pem(cert_pem.as_bytes()) {
            Ok(c) => c,
            Err(e) => {
                entry.error(format!("Invalid cert PEM: {}", e));
                return;
            }
        };
        if let Err(e) = pingora_core::tls::ext::ssl_use_certificate(ssl, &cert) {
            entry.error(format!("Failed to use cert: {}", e));
            return;
        }

        let key_pem = match edgion_tls.key_pem() {
            Ok(pem) => pem,
            Err(e) => {
                entry.error(format!("Failed to get key: {}", e));
                return;
            }
        };
        let key = match PKey::private_key_from_pem(key_pem.as_bytes()) {
            Ok(k) => k,
            Err(e) => {
                entry.error(format!("Invalid key PEM: {}", e));
                return;
            }
        };
        if let Err(e) = pingora_core::tls::ext::ssl_use_private_key(ssl, &key) {
            entry.error(format!("Failed to use key: {}", e));
            return;
        }

        if let Some(ref client_auth) = edgion_tls.spec.client_auth {
            if let Err(e) = self.configure_mtls(ssl, client_auth, edgion_tls) {
                entry.error(format!("mTLS config failed: {}", e));
                return;
            }
        }

        if let Some(min_version) = edgion_tls.spec.min_tls_version {
            if let Err(e) = self.configure_min_tls_version(ssl, min_version) {
                entry.error(format!("TLS version config failed: {}", e));
                return;
            }
        }

        if let Some(ref ciphers) = edgion_tls.spec.ciphers {
            if let Err(e) = self.configure_ciphers(ssl, ciphers) {
                entry.error(format!("Cipher config failed: {}", e));
            }
        }
    }

    /// Apply certificate from Gateway Listener TLS configuration (from Secret)
    fn apply_gateway_tls_cert(&self, ssl: &mut SslRef, gateway_tls: &GatewayTlsEntry, entry: &mut SslLogEntry) {
        entry.cert(format!(
            "Gateway:{}/{}/{}",
            gateway_tls.gateway_namespace, gateway_tls.gateway_name, gateway_tls.listener_name
        ));
        entry.mtls(false);

        let secret = if let Some(secrets) = &gateway_tls.secrets {
            if let Some(s) = secrets.first() {
                s.clone()
            } else {
                self.get_secret_from_store_or_error(gateway_tls, entry)
            }
        } else {
            self.get_secret_from_store_or_error(gateway_tls, entry)
        };

        self.apply_secret_to_ssl(ssl, &secret, entry);
    }

    /// Helper: Get Secret from global SecretStore (fallback for legacy behavior)
    fn get_secret_from_store_or_error(
        &self,
        gateway_tls: &GatewayTlsEntry,
        entry: &mut SslLogEntry,
    ) -> k8s_openapi::api::core::v1::Secret {
        let cert_ref = match gateway_tls.certificate_refs.first() {
            Some(r) => r,
            None => {
                entry.error("No certificate refs in Gateway TLS config");
                return k8s_openapi::api::core::v1::Secret::default();
            }
        };

        let secret_namespace = cert_ref.namespace.as_deref().unwrap_or(&gateway_tls.gateway_namespace);

        match get_secret_by_name(Some(secret_namespace), &cert_ref.name) {
            Some(s) => s,
            None => {
                entry.error(format!("Secret not found: {}/{}", secret_namespace, cert_ref.name));
                k8s_openapi::api::core::v1::Secret::default()
            }
        }
    }

    /// Helper: Extract cert/key from Secret and apply to SSL
    fn apply_secret_to_ssl(
        &self,
        ssl: &mut SslRef,
        secret: &k8s_openapi::api::core::v1::Secret,
        entry: &mut SslLogEntry,
    ) {
        let data = match &secret.data {
            Some(d) => d,
            None => {
                entry.error("Secret has no data");
                return;
            }
        };

        let cert_pem = match data.get(CERT) {
            Some(bytes) => match String::from_utf8(bytes.0.clone()) {
                Ok(s) => s,
                Err(e) => {
                    entry.error(format!("Invalid {} encoding: {}", CERT, e));
                    return;
                }
            },
            None => {
                entry.error(format!("Secret missing {}", CERT));
                return;
            }
        };

        let key_pem = match data.get(KEY) {
            Some(bytes) => match String::from_utf8(bytes.0.clone()) {
                Ok(s) => s,
                Err(e) => {
                    entry.error(format!("Invalid {} encoding: {}", KEY, e));
                    return;
                }
            },
            None => {
                entry.error(format!("Secret missing {}", KEY));
                return;
            }
        };

        let cert = match X509::from_pem(cert_pem.as_bytes()) {
            Ok(c) => c,
            Err(e) => {
                entry.error(format!("Invalid cert PEM from Secret: {}", e));
                return;
            }
        };
        if let Err(e) = pingora_core::tls::ext::ssl_use_certificate(ssl, &cert) {
            entry.error(format!("Failed to use cert: {}", e));
            return;
        }

        let key = match PKey::private_key_from_pem(key_pem.as_bytes()) {
            Ok(k) => k,
            Err(e) => {
                entry.error(format!("Invalid key PEM from Secret: {}", e));
                return;
            }
        };
        if let Err(e) = pingora_core::tls::ext::ssl_use_private_key(ssl, &key) {
            entry.error(format!("Failed to use key: {}", e));
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
