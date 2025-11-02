use anyhow::anyhow;
use pingora_core::listeners::tls::TlsSettings;
use pingora_core::listeners::TlsAccept;
use pingora_core::protocols::tls::TlsRef;
use pingora_core::tls::ssl::{NameType, SslRef};
use pingora_core::{Error as PingoraError, ErrorType};
use anyhow::Result;

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

    pub fn new_tls_settings_with_callback() -> Result<TlsSettings> {
        let callback = Box::new(TlsCallback::new());
        TlsSettings::with_callbacks(callback).map_err(|e|anyhow!("Failed to create TLS settings: {}", e))
    }

    async fn load_cert_from_sni(&self, ssl: &mut SslRef) -> Result<(), Box<PingoraError>>{
        println!("Loading TLS certificates");
        let sni = ssl.servername(NameType::HOST_NAME).
            map(|s| s.to_string()).
            ok_or_else(||PingoraError::explain(ErrorType::InvalidCert, "No SNI was provided"))?;

        println!("sni = {:?}", sni);



        Ok(())
    }
}



