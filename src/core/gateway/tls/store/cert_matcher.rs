#![allow(dead_code)]

use crate::core::common::matcher::HashHost;
use crate::types::err::EdError;
use crate::types::EdgionTls;
use anyhow::Result;
use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

struct TlsCertMatcherData {
    port_matcher: HashMap<u16, HashHost<Vec<Arc<EdgionTls>>>>,
    global_matcher: HashHost<Vec<Arc<EdgionTls>>>,
}

impl TlsCertMatcherData {
    fn new() -> Self {
        Self {
            port_matcher: HashMap::new(),
            global_matcher: HashHost::new(),
        }
    }
}

pub struct TlsCertMatcher {
    data: ArcSwap<TlsCertMatcherData>,
}

impl Default for TlsCertMatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl TlsCertMatcher {
    pub fn new() -> Self {
        Self {
            data: ArcSwap::from_pointee(TlsCertMatcherData::new()),
        }
    }

    /// Atomically replace both port-specific and global matchers.
    ///
    /// # Warning
    /// Do not call this method frequently. Maintain at least 100ms interval between calls.
    pub fn set(
        &self,
        port_matcher: HashMap<u16, HashHost<Vec<Arc<EdgionTls>>>>,
        global_matcher: HashHost<Vec<Arc<EdgionTls>>>,
    ) {
        self.data.store(Arc::new(TlsCertMatcherData {
            port_matcher,
            global_matcher,
        }));
    }

    /// Match with port dimension (preferred).
    /// 1. Try port-specific match
    /// 2. Fallback to global matcher (for EdgionTls without parentRefs)
    pub fn match_sni_with_port(&self, port: u16, sni: &str) -> Result<Arc<EdgionTls>, EdError> {
        let snapshot = self.data.load();

        if let Some(host_matcher) = snapshot.port_matcher.get(&port) {
            if let Some(tls_list) = host_matcher.get(sni) {
                if let Some(first) = tls_list.first() {
                    return Ok(first.clone());
                }
            }
        }

        let tls_list = snapshot.global_matcher.get(sni).cloned().unwrap_or_default();
        tls_list
            .first()
            .cloned()
            .ok_or_else(|| EdError::SniNotMatch(format!("port={}, sni={}", port, sni)))
    }

    /// Match without port (backward compat, searches all).
    /// 1. Search all port-specific matchers
    /// 2. Fallback to global matcher
    pub fn match_sni(&self, sni: &str) -> Result<Arc<EdgionTls>, EdError> {
        let snapshot = self.data.load();

        for host_matcher in snapshot.port_matcher.values() {
            if let Some(tls_list) = host_matcher.get(sni) {
                if let Some(first) = tls_list.first() {
                    return Ok(first.clone());
                }
            }
        }

        let tls_list = snapshot.global_matcher.get(sni).cloned().unwrap_or_default();
        tls_list
            .first()
            .cloned()
            .ok_or_else(|| EdError::SniNotMatch(sni.to_string()))
    }
}

pub static TLS_CERT_MATCHER: LazyLock<TlsCertMatcher> = LazyLock::new(TlsCertMatcher::new);

pub fn get_tls_cert_matcher() -> &'static TlsCertMatcher {
    &TLS_CERT_MATCHER
}

pub fn set_tls_cert_matcher(
    port_matcher: HashMap<u16, HashHost<Vec<Arc<EdgionTls>>>>,
    global_matcher: HashHost<Vec<Arc<EdgionTls>>>,
) -> Result<()> {
    get_tls_cert_matcher().set(port_matcher, global_matcher);
    Ok(())
}

pub fn match_sni(sni: &str) -> Result<Arc<EdgionTls>, EdError> {
    get_tls_cert_matcher().match_sni(sni)
}

pub fn match_sni_with_port(port: u16, sni: &str) -> Result<Arc<EdgionTls>, EdError> {
    get_tls_cert_matcher().match_sni_with_port(port, sni)
}
