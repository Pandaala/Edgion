use crate::core::conf_sync::conf_server::get_secret_by_name;
use crate::core::gateway::gateway::{match_gateway_tls, GatewayTlsEntry};
use crate::core::observe::ssl_log::{log_ssl, SslLogEntry};
use crate::core::tls::tls_cert_matcher::match_sni;
use crate::types::resources::edgion_gateway_config::EdgionGatewayConfig;
use crate::types::resources::edgion_tls::{ClientAuthConfig, ClientAuthMode, EdgionTls};
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

pub struct TlsCallback {
    edgion_gateway_config: Arc<EdgionGatewayConfig>,
}

#[async_trait::async_trait]
impl TlsAccept for TlsCallback {
    async fn certificate_callback(&self, ssl: &mut TlsRef) {
        let mut entry = SslLogEntry::new();
        self.load_cert_from_sni(ssl, &mut entry).await;
        log_ssl(&entry);
    }
}

impl TlsCallback {
    pub fn new(edgion_gateway_config: Arc<EdgionGatewayConfig>) -> Self {
        Self { edgion_gateway_config }
    }

    pub fn new_tls_settings_with_callback(
        edgion_gateway_config: Arc<EdgionGatewayConfig>,
        enable_http2: bool,
    ) -> Result<TlsSettings> {
        let callback = Box::new(TlsCallback::new(edgion_gateway_config));
        let mut settings =
            TlsSettings::with_callbacks(callback).map_err(|e| anyhow!("Failed to create TLS settings: {}", e))?;

        // Enable HTTP/2 support if requested
        if enable_http2 {
            settings.enable_h2();
        }

        Ok(settings)
    }

    /// Load certificate from SNI and populate log entry
    ///
    /// Uses a layered lookup strategy:
    /// 1. First, try to match SNI against EdgionTls resources
    /// 2. If not found, fallback to Gateway Listener TLS configurations (from Secret)
    async fn load_cert_from_sni(&self, ssl: &mut SslRef, entry: &mut SslLogEntry) {
        // Get SNI from SSL context, with fallback support
        let sni = match ssl.servername(NameType::HOST_NAME) {
            Some(s) => s.to_string(),
            None => {
                // Try to use fallback SNI from config
                if let Some(ref security_protect) = self.edgion_gateway_config.spec.security_protect {
                    if let Some(ref fallback) = security_protect.fallback_sni {
                        fallback.clone()
                    } else {
                        entry.error("No SNI provided and no fallback configured");
                        return;
                    }
                } else {
                    entry.error("No SNI provided and no security config");
                    return;
                }
            }
        };
        entry.sni(&sni);

        // Layer 1: Try to match against EdgionTls resources
        if let Ok(edgion_tls) = match_sni(&sni) {
            self.apply_edgion_tls_cert(ssl, &edgion_tls, entry);
            return;
        }

        // Layer 2: Fallback to Gateway Listener TLS configurations
        if let Ok(gateway_tls) = match_gateway_tls(&sni) {
            self.apply_gateway_tls_cert(ssl, &gateway_tls, entry);
            return;
        }

        entry.error(format!(
            "Certificate not found in EdgionTls or Gateway for SNI: {}",
            sni
        ));
    }

    /// Apply certificate from EdgionTls resource
    fn apply_edgion_tls_cert(&self, ssl: &mut SslRef, edgion_tls: &Arc<EdgionTls>, entry: &mut SslLogEntry) {
        // Record matched certificate
        let ns = edgion_tls.metadata.namespace.as_deref().unwrap_or("-");
        let name = edgion_tls.metadata.name.as_deref().unwrap_or("-");
        entry.cert(format!("EdgionTls:{}/{}", ns, name));

        // Record mTLS mode
        entry.mtls(edgion_tls.spec.client_auth.is_some());

        // Load certificate
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

        // Load private key
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

        // Configure mTLS
        if let Some(ref client_auth) = edgion_tls.spec.client_auth {
            if let Err(e) = self.configure_mtls(ssl, client_auth, edgion_tls) {
                entry.error(format!("mTLS config failed: {}", e));
                return;
            }
        }

        // Configure minimum TLS version
        if let Some(min_version) = edgion_tls.spec.min_tls_version {
            if let Err(e) = self.configure_min_tls_version(ssl, min_version) {
                entry.error(format!("TLS version config failed: {}", e));
                return;
            }
        }

        // Configure cipher list
        if let Some(ref ciphers) = edgion_tls.spec.ciphers {
            if let Err(e) = self.configure_ciphers(ssl, ciphers) {
                entry.error(format!("Cipher config failed: {}", e));
            }
        }
    }

    /// Apply certificate from Gateway Listener TLS configuration (from Secret)
    fn apply_gateway_tls_cert(&self, ssl: &mut SslRef, gateway_tls: &GatewayTlsEntry, entry: &mut SslLogEntry) {
        // Record source
        entry.cert(format!(
            "Gateway:{}/{}/{}",
            gateway_tls.gateway_namespace, gateway_tls.gateway_name, gateway_tls.listener_name
        ));

        // Gateway TLS doesn't support mTLS by default
        entry.mtls(false);

        // Priority 1: Use embedded Secret from GatewayTlsEntry (filled by Controller)
        let secret = if let Some(secrets) = &gateway_tls.secrets {
            if let Some(s) = secrets.first() {
                s.clone()
            } else {
                // Fall back to SecretStore lookup
                self.get_secret_from_store_or_error(gateway_tls, entry)
            }
        } else {
            // Fall back to SecretStore lookup
            self.get_secret_from_store_or_error(gateway_tls, entry)
        };

        // Extract and apply certificate from Secret
        self.apply_secret_to_ssl(ssl, &secret, entry);
    }

    /// Helper: Get Secret from global SecretStore (fallback for legacy behavior)
    fn get_secret_from_store_or_error(
        &self,
        gateway_tls: &GatewayTlsEntry,
        entry: &mut SslLogEntry,
    ) -> k8s_openapi::api::core::v1::Secret {
        // Get the first certificate ref (typically there's only one)
        let cert_ref = match gateway_tls.certificate_refs.first() {
            Some(r) => r,
            None => {
                entry.error("No certificate refs in Gateway TLS config");
                return k8s_openapi::api::core::v1::Secret::default();
            }
        };

        // Resolve Secret namespace (use Gateway namespace if not specified)
        let secret_namespace = cert_ref.namespace.as_deref().unwrap_or(&gateway_tls.gateway_namespace);

        // Load Secret from global store
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
        // Extract tls.crt and tls.key from Secret
        let data = match &secret.data {
            Some(d) => d,
            None => {
                entry.error("Secret has no data");
                return;
            }
        };

        let cert_pem = match data.get("tls.crt") {
            Some(bytes) => match String::from_utf8(bytes.0.clone()) {
                Ok(s) => s,
                Err(e) => {
                    entry.error(format!("Invalid tls.crt encoding: {}", e));
                    return;
                }
            },
            None => {
                entry.error("Secret missing tls.crt");
                return;
            }
        };

        let key_pem = match data.get("tls.key") {
            Some(bytes) => match String::from_utf8(bytes.0.clone()) {
                Ok(s) => s,
                Err(e) => {
                    entry.error(format!("Invalid tls.key encoding: {}", e));
                    return;
                }
            },
            None => {
                entry.error("Secret missing tls.key");
                return;
            }
        };

        // Parse and apply certificate
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

        // Parse and apply private key
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

        // Log success - secret name from metadata
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

        // Get CA certificate PEM
        let ca_pem = edgion_tls.ca_cert_pem().map_err(|e| {
            PingoraError::explain(
                ErrorType::InvalidCert,
                format!("Failed to get CA cert PEM for mTLS: {}", e),
            )
        })?;

        // Parse CA certificate
        let ca_cert = X509::from_pem(ca_pem.as_bytes())
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Invalid CA certificate PEM: {}", e)))?;

        // Create X509 store and add CA certificate
        let mut store_builder = X509StoreBuilder::new().map_err(|e| {
            PingoraError::explain(ErrorType::InvalidCert, format!("Failed to create X509 store: {}", e))
        })?;

        store_builder.add_cert(ca_cert).map_err(|e| {
            PingoraError::explain(ErrorType::InvalidCert, format!("Failed to add CA cert to store: {}", e))
        })?;

        let store = store_builder.build();

        // Set the CA store for client certificate verification
        ssl.set_verify_cert_store(store)
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Failed to set CA store: {}", e)))?;

        // Set verification mode based on mTLS mode
        let verify_mode = match client_auth.mode {
            ClientAuthMode::Terminate => {
                // Single-way TLS, no client certificate required
                tracing::debug!("mTLS mode: Terminate (single-way TLS)");
                return Ok(());
            }
            ClientAuthMode::Mutual => {
                // Mutual TLS: client certificate is mandatory
                tracing::debug!("mTLS mode: Mutual (client cert required)");
                SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT
            }
            ClientAuthMode::OptionalMutual => {
                // Optional mutual TLS: client certificate is optional
                tracing::debug!("mTLS mode: OptionalMutual (client cert optional)");
                SslVerifyMode::PEER
            }
        };

        // Set verification depth (BoringSSL only - OpenSSL uses different API)
        // SAFETY: verify_depth is validated to be in range 1-9 by cert_validator
        // u8 (1-9) can always be safely converted to u32
        #[cfg(feature = "boringssl")]
        {
            ssl.set_verify_depth(client_auth.verify_depth as u32);
            tracing::debug!("Set mTLS verification depth: {}", client_auth.verify_depth);
        }
        #[cfg(not(feature = "boringssl"))]
        {
            // Note: OpenSSL/Rustls have different APIs for setting verify depth
            // For now, we rely on the default depth which is typically sufficient
            tracing::debug!(
                "Verify depth configuration: {} (using backend default, explicit setting requires boringssl)",
                client_auth.verify_depth
            );
        }

        // Set verify mode with custom callback for SAN/CN whitelist (if configured)
        if client_auth.allowed_sans.is_some() || client_auth.allowed_cns.is_some() {
            tracing::debug!("Setting custom verify callback for SAN/CN whitelist");

            // Use backend_api unified interface, backend differences are centrally handled
            if let Err(e) = super::set_mtls_verify_callback(ssl, verify_mode, edgion_tls) {
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
            // No whitelist, use standard verify mode
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

        // Warn about deprecated TLS versions
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
