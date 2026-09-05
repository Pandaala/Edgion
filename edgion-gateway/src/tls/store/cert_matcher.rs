use arc_swap::ArcSwap;
use edgion_resources::listener_key::ListenerKey;
use edgion_resources::matcher::HashHost;
use edgion_resources::resources::edgion_tls::ResolvedListenerAttachment;
use edgion_resources::EdgionTls;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

static EMPTY_TLS_CANDIDATES: LazyLock<Arc<Vec<Arc<EdgionTls>>>> = LazyLock::new(|| Arc::new(Vec::new()));
pub(crate) const MAX_ATTACHMENT_CANDIDATES: usize = 64;

pub(crate) enum AttachmentMatchSlot {
    Ready(Arc<Vec<Arc<EdgionTls>>>),
    Overflow,
}

struct TlsCertMatcherData {
    listener_matcher: HashMap<ListenerKey, HashHost<Arc<Vec<Arc<EdgionTls>>>>>,
    // Exact attachment first, then at most 16 operator-bounded host patterns
    // per candidate. Avoid materializing attachment x hostname entries.
    attachment_matcher: HashMap<ResolvedListenerAttachment, AttachmentMatchSlot>,
}

/// One immutable matcher generation captured with a single ArcSwap load.
/// Handshake code uses this for both listener-SNI and exact-attachment lookup,
/// preventing a concurrent rebuild from mixing two controller generations.
pub struct TlsCertMatcherSnapshot {
    data: Arc<TlsCertMatcherData>,
}

impl TlsCertMatcherSnapshot {
    pub fn match_sni_candidates(&self, key: &ListenerKey, sni: &str) -> Arc<Vec<Arc<EdgionTls>>> {
        self.data
            .listener_matcher
            .get(key)
            .and_then(|matcher| matcher.get_rfc6125(sni))
            .cloned()
            .unwrap_or_else(|| EMPTY_TLS_CANDIDATES.clone())
    }

    pub fn match_attachment_sni(&self, attachment: &ResolvedListenerAttachment, sni: &str) -> Option<Arc<EdgionTls>> {
        let AttachmentMatchSlot::Ready(candidates) = self.data.attachment_matcher.get(attachment)? else {
            return None;
        };
        candidates
            .iter()
            .find(|candidate| {
                candidate
                    .spec
                    .hosts
                    .iter()
                    .any(|host| hostname_matches_rfc6125(host, sni))
            })
            .cloned()
    }
}

impl TlsCertMatcherData {
    fn new() -> Self {
        Self {
            listener_matcher: HashMap::new(),
            attachment_matcher: HashMap::new(),
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
    pub(crate) fn set(
        &self,
        listener_matcher: HashMap<ListenerKey, HashHost<Arc<Vec<Arc<EdgionTls>>>>>,
        attachment_matcher: HashMap<ResolvedListenerAttachment, AttachmentMatchSlot>,
    ) {
        self.data.store(Arc::new(TlsCertMatcherData {
            listener_matcher,
            attachment_matcher,
        }));
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

fn hostname_matches_rfc6125(pattern: &str, hostname: &str) -> bool {
    if pattern.eq_ignore_ascii_case(hostname) {
        return true;
    }
    let Some(suffix) = pattern.strip_prefix("*.") else {
        return false;
    };
    let Some(dot) = hostname.find('.') else {
        return false;
    };
    !hostname[..dot].is_empty() && hostname[dot + 1..].eq_ignore_ascii_case(suffix)
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

pub fn match_sni_candidates(key: &ListenerKey, sni: &str) -> Arc<Vec<Arc<EdgionTls>>> {
    get_tls_cert_matcher().match_sni_candidates(key, sni)
}

pub fn match_attachment_sni(attachment: &ResolvedListenerAttachment, sni: &str) -> Option<Arc<EdgionTls>> {
    get_tls_cert_matcher().match_attachment_sni(attachment, sni)
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::edgion_tls::{EdgionTlsSpec, GatewayAttachmentProof};
    use edgion_resources::resources::gateway::SecretObjectReference;
    use kube::api::ObjectMeta;

    fn tls(name: &str) -> Arc<EdgionTls> {
        Arc::new(EdgionTls {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: EdgionTlsSpec {
                parent_refs: None,
                hosts: vec!["example.com".to_string()],
                secret_ref: SecretObjectReference {
                    name: "server".to_string(),
                    namespace: None,
                    group: None,
                    kind: None,
                },
                client_auth: None,
                min_tls_version: None,
                ciphers: None,
                secret: None,
                resolved_listeners: None,
                resolved_listener_attachments: None,
                resolved_listener_attachment_index: Default::default(),
                resolved_log_labels: None,
            },
            status: None,
        })
    }

    #[test]
    fn exact_attachment_lookup_is_not_shadowed_by_more_than_64_unrelated_candidates() {
        let matcher = TlsCertMatcher::new();
        let listener_key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
        let attachment = ResolvedListenerAttachment {
            gateway_namespace: "default".to_string(),
            gateway_name: "winner".to_string(),
            listener_name: "https".to_string(),
            listener_key: listener_key.clone(),
            gateway_proof: GatewayAttachmentProof {
                instance: "uid-1".to_string(),
                generation: 1,
                epoch: "boot-1:1".to_string(),
                attachment_digest: "a".repeat(64),
            },
        };
        let unrelated = (0..65)
            .map(|index| tls(&format!("unrelated-{index}")))
            .collect::<Vec<_>>();
        let exact = tls("exact");

        let mut listener_hosts = HashHost::new();
        listener_hosts.insert("example.com", Arc::new(unrelated));
        matcher.set(
            HashMap::from([(listener_key, listener_hosts)]),
            HashMap::from([(
                attachment.clone(),
                AttachmentMatchSlot::Ready(Arc::new(vec![exact.clone()])),
            )]),
        );

        let selected = matcher
            .match_attachment_sni(&attachment, "example.com")
            .expect("exact attachment must remain indexed");
        assert!(Arc::ptr_eq(&selected, &exact));
    }

    #[test]
    fn one_snapshot_never_mixes_listener_and_attachment_generations() {
        let matcher = TlsCertMatcher::new();
        let listener_key = ListenerKey::new("0.0.0.0:443".parse().unwrap(), "HTTPS");
        let attachment = ResolvedListenerAttachment {
            gateway_namespace: "default".to_string(),
            gateway_name: "winner".to_string(),
            listener_name: "https".to_string(),
            listener_key: listener_key.clone(),
            gateway_proof: GatewayAttachmentProof {
                instance: "uid-1".to_string(),
                generation: 1,
                epoch: "boot-1:1".to_string(),
                attachment_digest: "a".repeat(64),
            },
        };
        let old = tls("old");
        let new = tls("new");
        let mut old_listener = HashHost::new();
        old_listener.insert("example.com", Arc::new(vec![old.clone()]));
        matcher.set(
            HashMap::from([(listener_key.clone(), old_listener)]),
            HashMap::from([(
                attachment.clone(),
                AttachmentMatchSlot::Ready(Arc::new(vec![old.clone()])),
            )]),
        );
        let snapshot = matcher.snapshot();

        let mut new_listener = HashHost::new();
        new_listener.insert("example.com", Arc::new(vec![new.clone()]));
        matcher.set(
            HashMap::from([(listener_key.clone(), new_listener)]),
            HashMap::from([(
                attachment.clone(),
                AttachmentMatchSlot::Ready(Arc::new(vec![new.clone()])),
            )]),
        );

        assert!(Arc::ptr_eq(
            snapshot
                .match_sni_candidates(&listener_key, "example.com")
                .first()
                .unwrap(),
            &old
        ));
        assert!(Arc::ptr_eq(
            &snapshot.match_attachment_sni(&attachment, "example.com").unwrap(),
            &old
        ));
        assert!(Arc::ptr_eq(
            &matcher.match_attachment_sni(&attachment, "example.com").unwrap(),
            &new
        ));
    }
}
