use super::compiled_tls::CompiledEdgionTls;
use super::tls_store::TlsSnapshotData;
use arc_swap::ArcSwap;
use edgion_algo::matcher::rfc6125_hostname::{lookup, matches};
use edgion_resources::listener_key::ListenerKey;
use edgion_resources::resources::edgion_tls::ResolvedListenerAttachment;
use std::sync::{Arc, LazyLock};

static EMPTY_TLS_CANDIDATES: LazyLock<Arc<[Arc<CompiledEdgionTls>]>> = LazyLock::new(|| Arc::from(Vec::new()));
pub(crate) const MAX_ATTACHMENT_CANDIDATES: usize = 64;

pub(crate) enum AttachmentMatchSlot {
    Ready(Arc<[Arc<CompiledEdgionTls>]>),
    Overflow,
}

/// One immutable matcher generation captured with a single ArcSwap load.
/// Handshake code uses this for both listener-SNI and exact-attachment lookup,
/// preventing a concurrent rebuild from mixing two controller generations.
pub struct TlsCertMatcherSnapshot {
    pub(super) data: Arc<TlsSnapshotData>,
}

impl TlsCertMatcherSnapshot {
    pub fn match_sni_candidates(&self, key: &ListenerKey, sni: &str) -> Arc<[Arc<CompiledEdgionTls>]> {
        self.data
            .listeners
            .get(key)
            .and_then(|bucket| lookup(sni, |host| bucket.hosts.get(host)))
            .map(|slot| slot.ordered.clone())
            .unwrap_or_else(|| EMPTY_TLS_CANDIDATES.clone())
    }

    /// Use canonical exact/wildcard fallback while skipping withdrawn owners.
    /// The native Gateway snapshot is captured by the caller for the handshake.
    pub fn match_eligible_sni(
        &self,
        key: &ListenerKey,
        sni: &str,
        eligible: impl Fn(&ResolvedListenerAttachment) -> bool,
    ) -> Option<Arc<CompiledEdgionTls>> {
        let bucket = self.data.listeners.get(key)?;
        lookup(sni, |host| {
            bucket.hosts.get(host).and_then(|slot| {
                slot.ordered.iter().find(|candidate| {
                    candidate
                        .tls
                        .spec
                        .resolved_listener_attachments
                        .iter()
                        .flatten()
                        .any(|attachment| &attachment.listener_key == key && eligible(attachment))
                })
            })
        })
        .cloned()
    }

    pub fn match_attachment_sni(
        &self,
        attachment: &ResolvedListenerAttachment,
        sni: &str,
    ) -> Option<Arc<CompiledEdgionTls>> {
        let AttachmentMatchSlot::Ready(candidates) = &self.data.attachments.get(attachment)?.slot else {
            return None;
        };
        candidates
            .iter()
            .find(|candidate| candidate.tls.spec.hosts.iter().any(|host| matches(host, sni)))
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

    /// Single-lookup convenience for tests only.
    ///
    /// Production code must take one [`TlsCertMatcher::snapshot`] and query it,
    /// so every lookup for a connection reads the same published generation. A
    /// per-call helper loads the ArcSwap again each time, which would let two
    /// lookups for one handshake straddle a concurrent rebuild.
    ///
    /// Uses RFC 6125 §6.4.3 single-label wildcard semantics — a cert registered for
    /// `*.example.com` matches SNI `foo.example.com` but NOT `a.b.example.com`. Route
    /// matching at `TlsRouteTable::match_route` keeps multi-label semantics per Gateway
    /// API §4.3 — the two layers diverge deliberately.
    #[cfg(test)]
    pub fn match_sni(&self, key: &ListenerKey, sni: &str) -> Option<Arc<CompiledEdgionTls>> {
        self.snapshot().match_sni_candidates(key, sni).first().cloned()
    }

    /// Single-lookup convenience for tests only; see [`TlsCertMatcher::match_sni`]
    /// for why production reads one snapshot instead.
    #[cfg(test)]
    pub fn match_attachment_sni(
        &self,
        attachment: &ResolvedListenerAttachment,
        sni: &str,
    ) -> Option<Arc<CompiledEdgionTls>> {
        self.snapshot().match_attachment_sni(attachment, sni)
    }

    pub fn snapshot(&self) -> TlsCertMatcherSnapshot {
        TlsCertMatcherSnapshot {
            data: self.data.load_full(),
        }
    }
}

pub(super) static TLS_CERT_MATCHER: LazyLock<Arc<TlsCertMatcher>> = LazyLock::new(|| Arc::new(TlsCertMatcher::new()));

pub fn get_tls_cert_matcher() -> &'static TlsCertMatcher {
    &TLS_CERT_MATCHER
}
