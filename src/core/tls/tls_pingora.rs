#![allow(dead_code)]

use crate::core::tls::tls_cert_matcher::match_sni;
use crate::core::tls::tls_store::get_global_tls_store;
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
use pingora_core::tls::x509::{X509, X509Ref, X509StoreContextRef};
use pingora_core::{Error as PingoraError, ErrorType};
use std::sync::Arc;

pub struct TlsCallback {
    edgion_gateway_config: Arc<EdgionGatewayConfig>,
}

#[async_trait::async_trait]
impl TlsAccept for TlsCallback {
    async fn certificate_callback(&self, _ssl: &mut TlsRef) {
        if let Err(e) = self.load_cert_from_sni(_ssl).await {
            eprintln!("Failed to load certificate: {}", e);
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

        // Set verification depth
        ssl.set_verify_depth(client_auth.verify_depth as u32);
        tracing::debug!("Set mTLS verification depth: {}", client_auth.verify_depth);

        // Log allowed SANs/CNs if configured
        if let Some(ref sans) = client_auth.allowed_sans {
            tracing::debug!("mTLS allowed SANs: {:?}", sans);
        }
        if let Some(ref cns) = client_auth.allowed_cns {
            tracing::debug!("mTLS allowed CNs: {:?}", cns);
        }

        // TODO: Custom verification callback is not working with Pingora's certificate_callback
        // The certificate_callback is called during ServerHello phase, but client certificate
        // verification happens later. BoringSSL requires the verify callback to be set at
        // SSL_CTX level (global), but we need per-SNI dynamic configuration.
        // 
        // Uncomment below when Pingora adds support for per-connection verify callbacks:
        //
        // let client_auth_clone = Arc::new(client_auth.clone());
        // ssl.set_verify_callback(verify_mode, move |preverify_ok, x509_ctx| {
        //     verify_client_cert_callback(preverify_ok, x509_ctx, &client_auth_clone)
        // });
        
        // Set the verify mode (basic verification without custom callback)
        ssl.set_verify(verify_mode);
        tracing::debug!("Set mTLS verify mode: {:?}", verify_mode);

        tracing::info!(
            "mTLS configured successfully for SNI with mode: {:?}, verify_depth: {}",
            client_auth.mode,
            client_auth.verify_depth
        );

        Ok(())
    }
    
}

/// Custom client certificate verification callback for mTLS
/// This is called during TLS handshake to validate client certificates
fn verify_client_cert_callback(
    preverify_ok: bool,
    x509_ctx: &mut X509StoreContextRef,
    client_auth: &ClientAuthConfig,
) -> bool {
    tracing::info!("mTLS verification callback invoked: preverify_ok={}, mode={:?}", preverify_ok, client_auth.mode);
    
    // 1. Check BoringSSL's basic verification result
    if !preverify_ok {
        tracing::warn!("Client certificate pre-verification failed by BoringSSL");
        // For OptionalMutual, allow connection even if verification fails
        return matches!(client_auth.mode, ClientAuthMode::OptionalMutual);
    }

    // 2. Get current certificate being verified
    let cert = match x509_ctx.current_cert() {
        Some(c) => c,
        None => {
            tracing::debug!("No client certificate provided");
            // Mutual mode requires a certificate
            return !matches!(client_auth.mode, ClientAuthMode::Mutual);
        }
    };

    // 3. Verify SAN whitelist (if configured)
    if let Some(ref allowed_sans) = client_auth.allowed_sans {
        if !verify_san_whitelist(cert, allowed_sans) {
            tracing::warn!("Client certificate SAN not in whitelist");
            return false;
        }
        tracing::debug!("Client certificate SAN verified against whitelist");
    }

    // 4. Verify CN whitelist (if configured)
    if let Some(ref allowed_cns) = client_auth.allowed_cns {
        if !verify_cn_whitelist(cert, allowed_cns) {
            tracing::warn!("Client certificate CN not in whitelist");
            return false;
        }
        tracing::debug!("Client certificate CN verified against whitelist");
    }

    tracing::debug!("Client certificate verification successful");
    true
}

/// Verify client certificate SAN against whitelist
fn verify_san_whitelist(cert: &X509Ref, allowed_sans: &[String]) -> bool {
    let sans = extract_sans_from_cert(cert);
    
    if sans.is_empty() {
        tracing::debug!("No SANs found in client certificate");
        return false;
    }
    
    // Check if any SAN matches the whitelist
    for san in &sans {
        if allowed_sans.contains(san) {
            tracing::debug!("Client certificate SAN '{}' matches whitelist", san);
            return true;
        }
    }
    
    tracing::debug!("Client certificate SANs {:?} do not match whitelist {:?}", sans, allowed_sans);
    false
}

/// Verify client certificate CN against whitelist
fn verify_cn_whitelist(cert: &X509Ref, allowed_cns: &[String]) -> bool {
    if let Some(cn) = extract_cn_from_cert(cert) {
        if allowed_cns.contains(&cn) {
            tracing::debug!("Client certificate CN '{}' matches whitelist", cn);
            return true;
        }
        tracing::debug!("Client certificate CN '{}' does not match whitelist {:?}", cn, allowed_cns);
        false
    } else {
        tracing::debug!("No CN found in client certificate");
        false
    }
}

/// Extract Subject Alternative Names from certificate
fn extract_sans_from_cert(cert: &X509Ref) -> Vec<String> {
    let mut sans = Vec::new();
    
    // Get Subject Alternative Name extension
    if let Some(san_ext) = cert.subject_alt_names() {
        for name in san_ext {
            // Extract DNS names
            if let Some(dns_name) = name.dnsname() {
                sans.push(dns_name.to_string());
            }
            // Extract email addresses
            if let Some(email) = name.email() {
                sans.push(email.to_string());
            }
            // Extract IP addresses
            if let Some(ip) = name.ipaddress() {
                if let Ok(ip_str) = std::str::from_utf8(ip) {
                    sans.push(ip_str.to_string());
                }
            }
        }
    }
    
    sans
}

/// Extract Common Name from certificate Subject
fn extract_cn_from_cert(cert: &X509Ref) -> Option<String> {
    let subject = cert.subject_name();
    
    // Find CN entry in subject
    for entry in subject.entries() {
        if entry.object().to_string() == "CN" {
            if let Ok(cn) = entry.data().as_utf8() {
                return Some(cn.to_string());
            }
        }
    }
    
    None
}
