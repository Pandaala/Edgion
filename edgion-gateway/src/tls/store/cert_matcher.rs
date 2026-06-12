use arc_swap::ArcSwap;
use edgion_resources::listener_key::ListenerKey;
use edgion_resources::matcher::HashHost;
use edgion_resources::EdgionTls;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

struct TlsCertMatcherData {
    listener_matcher: HashMap<ListenerKey, HashHost<Vec<Arc<EdgionTls>>>>,
}

impl TlsCertMatcherData {
    fn new() -> Self {
        Self {
            listener_matcher: HashMap::new(),
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

    /// Atomically replace listener-specific matchers.
    ///
    /// # Warning
    /// Do not call this method frequently. Maintain at least 100ms interval between calls.
    pub fn set(&self, listener_matcher: HashMap<ListenerKey, HashHost<Vec<Arc<EdgionTls>>>>) {
        self.data.store(Arc::new(TlsCertMatcherData { listener_matcher }));
    }

    /// Match SNI against certificates for a specific listener.
    ///
    /// Uses RFC 6125 §6.4.3 single-label wildcard semantics — a cert registered for
    /// `*.example.com` matches SNI `foo.example.com` but NOT `a.b.example.com`. Route
    /// matching at `TlsRouteTable::match_route` keeps multi-label semantics per Gateway
    /// API §4.3 — the two layers diverge deliberately.
    pub fn match_sni(&self, key: &ListenerKey, sni: &str) -> Option<Arc<EdgionTls>> {
        let snapshot = self.data.load();

        if let Some(host_matcher) = snapshot.listener_matcher.get(key) {
            if let Some(tls_list) = host_matcher.get_rfc6125(sni) {
                if let Some(first) = tls_list.first() {
                    return Some(first.clone());
                }
            }
        }
        None
    }
}

impl TlsCertMatcher {
    /// Number of listeners with certificate matchers configured.
    pub fn listener_count(&self) -> usize {
        self.data.load().listener_matcher.len()
    }
}

pub static TLS_CERT_MATCHER: LazyLock<TlsCertMatcher> = LazyLock::new(TlsCertMatcher::new);

pub fn get_tls_cert_matcher() -> &'static TlsCertMatcher {
    &TLS_CERT_MATCHER
}

pub fn match_sni(key: &ListenerKey, sni: &str) -> Option<Arc<EdgionTls>> {
    get_tls_cert_matcher().match_sni(key, sni)
}
