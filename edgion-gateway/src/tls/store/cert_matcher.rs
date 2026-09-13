use super::tls_store::TlsSnapshotData;
use arc_swap::ArcSwap;
use edgion_algo::matcher::rfc6125_hostname::{lookup, matches};
use edgion_resources::listener_key::ListenerKey;
use edgion_resources::resources::edgion_tls::ResolvedListenerAttachment;
use edgion_resources::EdgionTls;
use std::sync::{Arc, LazyLock};

static EMPTY_TLS_CANDIDATES: LazyLock<Arc<Vec<Arc<EdgionTls>>>> = LazyLock::new(|| Arc::new(Vec::new()));
pub(crate) const MAX_ATTACHMENT_CANDIDATES: usize = 64;

pub(crate) enum AttachmentMatchSlot {
    Ready(Arc<Vec<Arc<EdgionTls>>>),
    Overflow,
}

/// One immutable matcher generation captured with a single ArcSwap load.
/// Handshake code uses this for both listener-SNI and exact-attachment lookup,
/// preventing a concurrent rebuild from mixing two controller generations.
pub struct TlsCertMatcherSnapshot {
    pub(super) data: Arc<TlsSnapshotData>,
}

impl TlsCertMatcherSnapshot {
    pub fn match_sni_candidates(&self, key: &ListenerKey, sni: &str) -> Arc<Vec<Arc<EdgionTls>>> {
        self.data
            .listeners
            .get(key)
            .and_then(|bucket| lookup(sni, |host| bucket.hosts.get(host)))
            .map(|slot| slot.ordered.clone())
            .unwrap_or_else(|| EMPTY_TLS_CANDIDATES.clone())
    }

    pub fn match_attachment_sni(&self, attachment: &ResolvedListenerAttachment, sni: &str) -> Option<Arc<EdgionTls>> {
        let AttachmentMatchSlot::Ready(candidates) = &self.data.attachments.get(attachment)?.slot else {
            return None;
        };
        candidates
            .iter()
            .find(|candidate| candidate.spec.hosts.iter().any(|host| matches(host, sni)))
            .cloned()
    }
}

pub struct TlsCertMatcher {
    pub(super) data: ArcSwap<TlsSnapshotData>,
}

impl Default for TlsCertMatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl TlsCertMatcher {
    pub fn new() -> Self {
        Self {
            data: ArcSwap::from_pointee(TlsSnapshotData::default()),
        }
    }

    /// Match SNI against certificates for a specific listener.
    ///
    /// Uses RFC 6125 §6.4.3 single-label wildcard semantics — a cert registered for
    /// `*.example.com` matches SNI `foo.example.com` but NOT `a.b.example.com`. Route
    /// matching at `TlsRouteTable::match_route` keeps multi-label semantics per Gateway
    /// API §4.3 — the two layers diverge deliberately.
    pub fn match_sni(&self, key: &ListenerKey, sni: &str) -> Option<Arc<EdgionTls>> {
        self.match_sni_candidates(key, sni).first().cloned()
    }

    /// Return the complete immutable winner slot for an SNI match. Gateway
    /// frontend TLS uses this to select an EdgionTls candidate by exact logical
    /// attachment instead of letting an unrelated first entry shadow it.
    pub fn match_sni_candidates(&self, key: &ListenerKey, sni: &str) -> Arc<Vec<Arc<EdgionTls>>> {
        self.snapshot().match_sni_candidates(key, sni)
    }

    /// Match within the exact logical Gateway attachment. This index is built
    /// in the same immutable generation as the ordinary listener matcher, so
    /// unrelated listener candidates cannot shadow or evict an exact overlay.
    pub fn match_attachment_sni(&self, attachment: &ResolvedListenerAttachment, sni: &str) -> Option<Arc<EdgionTls>> {
        self.snapshot().match_attachment_sni(attachment, sni)
    }

    pub fn snapshot(&self) -> TlsCertMatcherSnapshot {
        TlsCertMatcherSnapshot {
            data: self.data.load_full(),
        }
    }
}

impl TlsCertMatcher {
    /// Number of listeners with certificate matchers configured.
    pub fn listener_count(&self) -> usize {
        self.snapshot().listener_count()
    }
}

pub(super) static TLS_CERT_MATCHER: LazyLock<Arc<TlsCertMatcher>> = LazyLock::new(|| Arc::new(TlsCertMatcher::new()));

pub fn get_tls_cert_matcher() -> &'static TlsCertMatcher {
    &TLS_CERT_MATCHER
}

pub fn match_sni(key: &ListenerKey, sni: &str) -> Option<Arc<EdgionTls>> {
    get_tls_cert_matcher().match_sni(key, sni)
}

pub fn match_sni_candidates(key: &ListenerKey, sni: &str) -> Arc<Vec<Arc<EdgionTls>>> {
    get_tls_cert_matcher().match_sni_candidates(key, sni)
}

pub fn match_attachment_sni(attachment: &ResolvedListenerAttachment, sni: &str) -> Option<Arc<EdgionTls>> {
    get_tls_cert_matcher().match_attachment_sni(attachment, sni)
}
