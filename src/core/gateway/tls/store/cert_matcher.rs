#![allow(dead_code)]

use crate::core::common::matcher::HashHost;
use crate::types::EdgionTls;
use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

struct TlsCertMatcherData {
    port_matcher: HashMap<u16, HashHost<Vec<Arc<EdgionTls>>>>,
}

impl TlsCertMatcherData {
    fn new() -> Self {
        Self {
            port_matcher: HashMap::new(),
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

    /// Atomically replace port-specific matchers.
    ///
    /// # Warning
    /// Do not call this method frequently. Maintain at least 100ms interval between calls.
    pub fn set(
        &self,
        port_matcher: HashMap<u16, HashHost<Vec<Arc<EdgionTls>>>>,
    ) {
        self.data.store(Arc::new(TlsCertMatcherData { port_matcher }));
    }

    /// Match with port dimension.
    pub fn match_sni_with_port(&self, port: u16, sni: &str) -> Option<Arc<EdgionTls>> {
        let snapshot = self.data.load();

        if let Some(host_matcher) = snapshot.port_matcher.get(&port) {
            if let Some(tls_list) = host_matcher.get(sni) {
                if let Some(first) = tls_list.first() {
                    return Some(first.clone());
                }
            }
        }
        None
    }
}

pub static TLS_CERT_MATCHER: LazyLock<TlsCertMatcher> = LazyLock::new(TlsCertMatcher::new);

pub fn get_tls_cert_matcher() -> &'static TlsCertMatcher {
    &TLS_CERT_MATCHER
}

pub fn set_tls_cert_matcher(port_matcher: HashMap<u16, HashHost<Vec<Arc<EdgionTls>>>>) -> anyhow::Result<()> {
    get_tls_cert_matcher().set(port_matcher);
    Ok(())
}

pub fn match_sni_with_port(port: u16, sni: &str) -> Option<Arc<EdgionTls>> {
    get_tls_cert_matcher().match_sni_with_port(port, sni)
}
