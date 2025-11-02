#[derive(Debug, Clone)]
pub struct TlsWithSecret {
    pub tls: EdgionTls,
    pub secret: Secret,
}

impl TlsWithSecret {
    pub fn new(tls: EdgionTls, secret: Secret) -> Self {
        Self { tls, secret }
    }

    pub fn cert_pem(&self) -> Result<String, EdError> {
        let secret = self.secret.get_secret().await?;
        let cert_pem = secret
            .data
            .get("tls.crt")
            .ok_or(EdError::NotFound("tls.crt not found"))?;
        Ok(String::from_utf8(cert_pem.clone())?)
    }

    pub fn key_pem(&self) -> Result<String, EdError> {
        let secret = self.secret.get_secret().await?;
        let key_pem = secret
            .data
            .get("tls.key")
            .ok_or(EdError::NotFound("tls.key not found"))?;
        Ok(String::from_utf8(key_pem.clone())?)
    }
}
