use crate::core::host_match::HashHost;
use crate::types::err::EdError;
use crate::types::EdgionTls;
use anyhow::Result;
use arc_swap::ArcSwap;
use k8s_openapi::api::core::v1::Secret;
use std::sync::{Arc, OnceLock};

#[derive(Debug, Clone)]
pub struct TlsWithSecret {
    pub tls: EdgionTls,
    pub secret: Secret,
}

impl TlsWithSecret {
    pub fn new(tls: EdgionTls, secret: Secret) -> Self {
        Self { tls, secret }
    }

    pub fn cert(&self) -> Result<String> {
        let data = self
            .secret
            .data
            .as_ref()
            .ok_or_else(|| (anyhow::anyhow!("Secret data not found")))?;
        let cert_pem = data
            .get("tls.crt")
            .ok_or_else(|| (anyhow::anyhow!("Secret data tls.crt not found")))?;
        String::from_utf8(cert_pem.0.clone()).map_err(|e| anyhow::anyhow!(e))
    }

    pub fn key(&self) -> Result<String> {
        let data = self
            .secret
            .data
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Secret data not found"))?;
        let key_pen = data
            .get("tls.key")
            .ok_or_else(|| anyhow::anyhow!("Secret data tls.key not found"))?;
        String::from_utf8(key_pen.0.clone()).map_err(|e| anyhow::anyhow!(e))
    }
}

pub struct TlsCertMatcher {
    matcher: ArcSwap<HashHost<Vec<Arc<TlsWithSecret>>>>,
}

impl TlsCertMatcher {
    pub fn new() -> Self {
        Self {
            matcher: ArcSwap::from_pointee(HashHost::new()),
        }
    }

    /// Set the entire certificate matcher
    /// This replaces all existing certificates with the provided matcher
    pub fn set(&self, matcher: HashHost<Vec<Arc<TlsWithSecret>>>) {
        self.matcher.store(Arc::new(matcher));
    }

    pub fn match_sni(&self, sni: &str) -> Result<Arc<TlsWithSecret>, EdError> {
        // Lock-free read: just load the Arc pointer atomically
        let snapshot = self.matcher.load();
        let tls_list = snapshot.get(sni).cloned().unwrap_or_default();

        if tls_list.is_empty() {
            return Err(EdError::SniNotMatch(sni.to_string()));
        }
        if let Some(t) = tls_list.first() {
            Ok(t.clone())
        } else {
            Err(EdError::SniNotMatch(sni.to_string()))
        }
    }
}

pub static TLS_CERT_MATCHER: OnceLock<TlsCertMatcher> = OnceLock::new();

pub fn init_tls_cert_matcher() -> Result<()> {
    let tls_cert_matcher = TlsCertMatcher::new();
    TLS_CERT_MATCHER
        .set(tls_cert_matcher)
        .map_err(|_| anyhow::anyhow!("TLS cert matcher already initialized"))?;
    Ok(())
}

pub fn get_tls_cert_matcher() -> Result<&'static TlsCertMatcher> {
    TLS_CERT_MATCHER
        .get()
        .ok_or_else(|| anyhow::anyhow!("TLS cert matcher not initialized"))
}

pub fn set_tls_cert_matcher(matcher: HashHost<Vec<Arc<TlsWithSecret>>>) -> Result<()> {
    let tls_cert_matcher = get_tls_cert_matcher()?;
    tls_cert_matcher.set(matcher);
    Ok(())
}

pub fn match_sni(sni: &str) -> Result<Arc<TlsWithSecret>, EdError> {
    let tls_cert_matcher =
        get_tls_cert_matcher().map_err(|e| EdError::InternalError(e.to_string()))?;
    tls_cert_matcher.match_sni(sni)
}
