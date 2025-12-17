#![allow(dead_code)]

use crate::core::tls::tls_cert_matcher::match_sni;
use anyhow::anyhow;
use anyhow::Result;
use pingora_core::listeners::tls::TlsSettings;
use pingora_core::listeners::TlsAccept;
use pingora_core::protocols::tls::TlsRef;
use pingora_core::tls::pkey::PKey;
use pingora_core::tls::ssl::{NameType, SslRef};
use pingora_core::tls::x509::X509;
use pingora_core::{Error as PingoraError, ErrorType};

pub struct TlsCallback {}

#[async_trait::async_trait]
impl TlsAccept for TlsCallback {
    async fn certificate_callback(&self, _ssl: &mut TlsRef) {
        if let Err(e) = self.load_cert_from_sni(_ssl).await {
            eprintln!("Failed to load certificate: {}", e);
        }
    }
}

impl TlsCallback {
    pub fn new() -> Self {
        Self {}
    }

    pub fn new_tls_settings_with_callback(enable_http2: bool) -> Result<TlsSettings> {
        let callback = Box::new(TlsCallback::new());
        let mut settings = TlsSettings::with_callbacks(callback)
            .map_err(|e| anyhow!("Failed to create TLS settings: {}", e))?;
        
        // Enable HTTP/2 support if requested
        if enable_http2 {
            settings.enable_h2();
        }
        
        Ok(settings)
    }

    async fn load_cert_from_sni(&self, ssl: &mut SslRef) -> Result<(), Box<PingoraError>> {
        println!("Loading TLS certificates");

        // Get SNI from SSL context
        let sni = ssl
            .servername(NameType::HOST_NAME)
            .map(|s| s.to_string())
            .ok_or_else(|| PingoraError::explain(ErrorType::InvalidCert, "No SNI was provided"))?;

        println!("SNI = {:?}", sni);

        // Match certificate by SNI
        let tls_with_secret = match_sni(&sni)
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Certificate not found: {}", e)))?;

        // Load certificate
        let cert_pem = tls_with_secret
            .cert_pem()
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Failed to get cert PEM: {}", e)))?;
        let cert = X509::from_pem(cert_pem.as_bytes())
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Invalid certificate PEM: {}", e)))?;

        pingora_core::tls::ext::ssl_use_certificate(ssl, &cert)
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Failed to use certificate: {}", e)))?;

        // Load private key
        let key_pem = tls_with_secret
            .key_pem()
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Failed to get key PEM: {}", e)))?;
        let key = PKey::private_key_from_pem(key_pem.as_bytes())
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Invalid private key PEM: {}", e)))?;

        pingora_core::tls::ext::ssl_use_private_key(ssl, &key)
            .map_err(|e| PingoraError::explain(ErrorType::InvalidCert, format!("Failed to use private key: {}", e)))?;

        println!("Successfully loaded TLS certificate and key for SNI: {}", sni);

        Ok(())
    }
}
