#![allow(dead_code)]

use crate::core::matcher::HashHost;
use crate::types::err::EdError;
use crate::types::EdgionTls;
use anyhow::Result;
use arc_swap::ArcSwap;
use std::sync::{Arc, LazyLock};

pub struct TlsCertMatcher {
    matcher: ArcSwap<HashHost<Vec<Arc<EdgionTls>>>>,
}

impl TlsCertMatcher {
    pub fn new() -> Self {
        Self {
            matcher: ArcSwap::from_pointee(HashHost::new()),
        }
    }

    /// Set the entire certificate matcher
    /// This replaces all existing certificates with the provided matcher
    ///
    /// # Warning
    /// Do not call this method frequently. Maintain at least 100ms interval between calls.
    pub fn set(&self, matcher: HashHost<Vec<Arc<EdgionTls>>>) {
        self.matcher.store(Arc::new(matcher));
    }

    pub fn match_sni(&self, sni: &str) -> Result<Arc<EdgionTls>, EdError> {
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

pub static TLS_CERT_MATCHER: LazyLock<TlsCertMatcher> = 
    LazyLock::new(|| TlsCertMatcher::new());

pub fn get_tls_cert_matcher() -> &'static TlsCertMatcher {
    &TLS_CERT_MATCHER
}

pub fn set_tls_cert_matcher(matcher: HashHost<Vec<Arc<EdgionTls>>>) -> Result<()> {
    let tls_cert_matcher = get_tls_cert_matcher();
    tls_cert_matcher.set(matcher);
    Ok(())
}

pub fn match_sni(sni: &str) -> Result<Arc<EdgionTls>, EdError> {
    let tls_cert_matcher = get_tls_cert_matcher();
    tls_cert_matcher.match_sni(sni)
}
