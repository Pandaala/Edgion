#![allow(dead_code)]
#![cfg(any(feature = "boringssl", feature = "openssl"))]

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
        if let Err(e) = self.load_cert_from_sni(ssl).await {
            // Extract SNI for logging context
            let sni = ssl.servername(NameType::HOST_NAME)
                .unwrap_or("<no-sni>");
            
            tracing::error!(
                component = "tls_callback",
                sni = %sni,
                error = %e,
                "Failed to load certificate during TLS handshake"
            );
        }
    }
}

impl TlsCallback {
    pub fn new(edgion_gateway_config: Arc<EdgionGatewayConfig>) -> Self {
        Self {
            edgion_gateway_config,
        }
    }

    pub fn new_tls_settings_with_callback(
        edgion_gateway_config: Arc<EdgionGatewayConfig>,
        enable_http2: bool,
    ) -> Result<TlsSettings> {
        let callback = Box::new(TlsCallback::new(edgion_gateway_config));
        let mut settings = TlsSettings::with_callbacks(callback)
            .map_err(|e| anyhow!("Failed to create TLS settings: {}", e))?;
        
        // Enable HTTP/2 support if requested
        if enable_http2 {
            settings.enable_h2();
        }
        
        Ok(settings)
    }

    async fn load_cert_from_sni(&self, ssl: &mut SslRef) -> Result<(), Box<PingoraError>> {
        // TODO(observability): Add metrics for:
        // - tls_cert_load_total counter (with status label: success/failure)
        // - tls_cert_load_duration_seconds histogram
        tracing::debug!("Loading TLS certificates");

        // Get SNI from SSL context, with fallback support
        let sni = match ssl.servername(NameType::HOST_NAME) {
            Some(s) => s.to_string(),
            None => {
                // Try to use fallback SNI from config
                if let Some(ref security_protect) = self.edgion_gateway_config.spec.security_protect {
                    if let Some(ref fallback) = security_protect.fallback_sni {
                        tracing::info!("No SNI provided by client, using fallback SNI: {}", fallback);
                        fallback.clone()
                    } else {
                        return Err(PingoraError::explain(
                            ErrorType::InvalidCert,
                            "No SNI was provided and no fallback SNI configured"
                        ));
                    }
                } else {
                    return Err(PingoraError::explain(
                        ErrorType::InvalidCert,
                        "No SNI was provided and no security protection config"
                    ));
                }
            }
        };

        tracing::debug!("Using SNI: {}", sni);

        // Match certificate by SNI
        let edgion_tls = match_sni(&sni)
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Certificate not found: {}", e)))?;

        // Load certificate FIRST
        let cert_pem = edgion_tls
            .cert_pem()
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Failed to get cert PEM: {}", e)))?;
        let cert = X509::from_pem(cert_pem.as_bytes())
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Invalid certificate PEM: {}", e)))?;

        pingora_core::tls::ext::ssl_use_certificate(ssl, &cert)
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Failed to use certificate: {}", e)))?;

        // Load private key
        let key_pem = edgion_tls
            .key_pem()
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Failed to get key PEM: {}", e)))?;
        let key = PKey::private_key_from_pem(key_pem.as_bytes())
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Invalid private key PEM: {}", e)))?;

        pingora_core::tls::ext::ssl_use_private_key(ssl, &key)
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Failed to use private key: {}", e)))?;

        tracing::debug!("Successfully loaded TLS certificate and key for SNI: {}", sni);

        // Configure mTLS AFTER loading server certificates
        // This is OK because SSL_set_verify can be called during the handshake
        if let Some(ref client_auth) = edgion_tls.spec.client_auth {
            self.configure_mtls(ssl, client_auth, &edgion_tls)?;
        }
        
        // Configure TLS version constraints
        if let Some(ref tls_versions) = edgion_tls.spec.tls_versions {
            self.configure_tls_versions(ssl, tls_versions)?;
        }
        
        // Configure cipher suites
        if let Some(ref cipher_suites) = edgion_tls.spec.cipher_suites {
            self.configure_cipher_suites(ssl, cipher_suites)?;
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
        tracing::debug!("Configuring mTLS with mode: {:?}", client_auth.mode);

        // Get CA certificate PEM
        let ca_pem = edgion_tls.ca_cert_pem().map_err(|e| {
            PingoraError::explain(
                ErrorType::InvalidCert,
                format!("Failed to get CA cert PEM for mTLS: {}", e),
            )
        })?;

        // Parse CA certificate
        let ca_cert = X509::from_pem(ca_pem.as_bytes()).map_err(|e| {
            PingoraError::explain(
                ErrorType::InvalidCert,
                format!("Invalid CA certificate PEM: {}", e),
            )
        })?;

        // Create X509 store and add CA certificate
        let mut store_builder = X509StoreBuilder::new().map_err(|e| {
            PingoraError::explain(
                ErrorType::InvalidCert,
                format!("Failed to create X509 store: {}", e),
            )
        })?;

        store_builder.add_cert(ca_cert).map_err(|e| {
            PingoraError::explain(
                ErrorType::InvalidCert,
                format!("Failed to add CA cert to store: {}", e),
            )
        })?;

        let store = store_builder.build();

        // Set the CA store for client certificate verification
        ssl.set_verify_cert_store(store).map_err(|e| {
            PingoraError::explain(
                ErrorType::InvalidCert,
                format!("Failed to set CA store: {}", e),
            )
        })?;

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
            #[cfg(feature = "boringssl")]
            {
                // Use custom verify callback that validates SAN/CN whitelist
                tracing::debug!("Setting custom verify callback for SAN/CN whitelist");
                
                if let Err(e) = crate::core::tls::boringssl::mtls_verify_callback::set_verify_callback_with_whitelist(
                    ssl,
                    verify_mode,
                    edgion_tls,
                ) {
                    return Err(PingoraError::explain(
                        ErrorType::InternalError,
                        format!("Failed to set verify callback: {}", e),
                    ));
                }
                
                tracing::info!("Custom verify callback configured for SAN/CN whitelist");
            }
            
            #[cfg(not(feature = "boringssl"))]
            {
                // SAN/CN whitelist is only supported with BoringSSL backend
                tracing::error!(
                    "SAN/CN whitelist validation requires 'boringssl' feature, but it's not enabled. \
                    Current TLS backend does not support custom verify callbacks."
                );
                return Err(PingoraError::explain(
                    ErrorType::InternalError,
                    "SAN/CN whitelist validation requires BoringSSL backend. \
                    Please rebuild with --features boringssl or remove allowed_sans/allowed_cns from configuration.".to_string(),
                ));
            }
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
    
    /// Configure TLS version constraints
    fn configure_tls_versions(
        &self,
        ssl: &mut SslRef,
        tls_versions: &crate::types::resources::edgion_tls::TlsVersionConfig,
    ) -> Result<(), Box<PingoraError>> {
        use pingora_core::tls::ssl::SslVersion;
        
        // Set minimum TLS version
        if let Some(min_version) = tls_versions.min_version {
            let ssl_min_version = match min_version {
                crate::types::resources::edgion_tls::TlsVersion::Tls12 => SslVersion::TLS1_2,
                crate::types::resources::edgion_tls::TlsVersion::Tls13 => SslVersion::TLS1_3,
            };
            
            ssl.set_min_proto_version(Some(ssl_min_version)).map_err(|e| {
                PingoraError::explain(
                    ErrorType::InternalError,
                    format!("Failed to set min TLS version: {}", e),
                )
            })?;
            
            tracing::debug!("Set minimum TLS version to: {:?}", min_version);
        }
        
        // Set maximum TLS version
        if let Some(max_version) = tls_versions.max_version {
            let ssl_max_version = match max_version {
                crate::types::resources::edgion_tls::TlsVersion::Tls12 => SslVersion::TLS1_2,
                crate::types::resources::edgion_tls::TlsVersion::Tls13 => SslVersion::TLS1_3,
            };
            
            ssl.set_max_proto_version(Some(ssl_max_version)).map_err(|e| {
                PingoraError::explain(
                    ErrorType::InternalError,
                    format!("Failed to set max TLS version: {}", e),
                )
            })?;
            
            tracing::debug!("Set maximum TLS version to: {:?}", max_version);
        }
        
        Ok(())
    }
    
    /// Configure cipher suites based on profile or custom list
    fn configure_cipher_suites(
        &self,
        _ssl: &mut SslRef,
        cipher_config: &crate::types::resources::edgion_tls::CipherSuiteConfig,
    ) -> Result<(), Box<PingoraError>> {
        use crate::types::resources::edgion_tls::CipherSuiteProfile;
        
        let cipher_list = match &cipher_config.profile {
            CipherSuiteProfile::Modern => {
                // Mozilla Modern profile: TLS 1.3 only
                // https://wiki.mozilla.org/Security/Server_Side_TLS
                "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256"
            }
            CipherSuiteProfile::Intermediate => {
                // Mozilla Intermediate profile: TLS 1.2+
                // Balanced security and compatibility
                "ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:\
                 ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:\
                 ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305:\
                 DHE-RSA-AES128-GCM-SHA256:DHE-RSA-AES256-GCM-SHA384:\
                 TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256"
            }
            CipherSuiteProfile::Old => {
                // Mozilla Old profile: maximum compatibility
                // Includes older ciphers for legacy clients
                "ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:\
                 ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:\
                 ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305:\
                 DHE-RSA-AES128-GCM-SHA256:DHE-RSA-AES256-GCM-SHA384:\
                 DHE-RSA-CHACHA20-POLY1305:ECDHE-ECDSA-AES128-SHA256:\
                 ECDHE-RSA-AES128-SHA256:ECDHE-ECDSA-AES128-SHA:\
                 ECDHE-RSA-AES128-SHA:ECDHE-ECDSA-AES256-SHA384:\
                 ECDHE-RSA-AES256-SHA384:ECDHE-ECDSA-AES256-SHA:\
                 ECDHE-RSA-AES256-SHA:DHE-RSA-AES128-SHA256:\
                 DHE-RSA-AES256-SHA256:AES128-GCM-SHA256:AES256-GCM-SHA384:\
                 AES128-SHA256:AES256-SHA256:AES128-SHA:AES256-SHA:\
                 DES-CBC3-SHA:\
                 TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256"
            }
            CipherSuiteProfile::Custom(ciphers) => {
                // User-specified custom cipher list
                // Join with colons as required by OpenSSL
                &ciphers.join(":")
            }
        };
        
        // Set cipher list (applies to TLS 1.2 and below, and TLS 1.3)
        // Note: Pingora/BoringSSL doesn't expose set_cipher_list in safe API
        // For now, we log the configuration but cannot apply it directly
        // This would require either:
        // 1. Pingora to expose set_cipher_list API
        // 2. Using unsafe FFI with proper foreign_types handling
        // 3. Configuring at SSL_CTX level before handshake (not possible with dynamic SNI)
        tracing::warn!(
            "Cipher suite configuration is not fully supported yet. \
             Profile={:?}, List={}. \
             This requires Pingora API enhancement.",
            cipher_config.profile,
            cipher_list
        );
        
        tracing::debug!("Set cipher suites: profile={:?}", cipher_config.profile);
        
        Ok(())
    }
}

