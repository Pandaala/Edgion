//! Gateway TLS Matcher
//!
//! Provides TLS certificate and frontend client-auth decisions from Gateway
//! Listener configurations. A matched frontend decision owns the connection;
//! specifically, `FrontendMtlsOutcome::StrictReject` rejects before any
//! EdgionTls overlay is considered. `StrictEffective` remains usable, and an
//! EdgionTls serving-certificate overlay is accepted only when its resolved
//! attachment identifies that exact winning Gateway listener. When the Gateway
//! entry has no frontend decision, ordinary EdgionTls matching retains
//! precedence and this matcher supplies the Gateway certificate fallback.
//!
//! ## Architecture
//!
//! The matcher uses a two-layer structure: Port -> (SNI -> TLSEntry)
//! This supports Gateway API semantics where the same hostname on different
//! ports can have different certificates.
//!
//! ## Performance Optimization
//!
//! Most users don't configure TLS in Gateway, so the inner `Option` provides a
//! fast path: when no Gateway TLS is configured, lookups return immediately
//! without touching any HashMap.

use arc_swap::ArcSwapOption;
use edgion_algo::matcher::rfc6125_hostname::Rfc6125HostnameMatcher;
use edgion_common::cert_util::LeafValidity;
use edgion_common::link_sys_core::fingerprint::FingerprintHasher;
use edgion_resources::listener_key::{resolve_listener_addr, ListenerKey};
use edgion_resources::prelude_resources::Gateway;
use edgion_resources::resources::edgion_tls::ResolvedListenerAttachment;
use edgion_resources::resources::gateway::{
    gateway_attachment_proof, FrontendCASourceKind, FrontendMatcherEligibility, FrontendTLSValidation,
    ResolvedFrontendCAReference, ResolvedServingCertificateReference, SecretObjectReference,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use kube::ResourceExt;
use pingora_core::tls::pkey::{Id, PKey, Private};
use pingora_core::tls::x509::store::{X509Store, X509StoreBuilder};
use pingora_core::tls::x509::X509;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

const MAX_COLLISION_PARTICIPANTS: usize = 64;

/// Gateway TLS entry containing certificate references and resolved secrets
#[derive(Clone)]
pub struct GatewayTlsEntry {
    /// Complete immutable logical attachment key, compiled on matcher rebuild
    /// and borrowed directly by the handshake path.
    pub attachment: ResolvedListenerAttachment,
    /// Gateway `metadata.creationTimestamp` — primary tiebreak key when two
    /// Gateways collide on the same hostname (oldest wins, per the Gateway API
    /// route-conflict convention; mirrors `BackendTLSPolicyStore` precedence).
    pub gateway_creation_ts: Option<Time>,
    /// References to Secrets containing certificates
    pub certificate_refs: Vec<SecretObjectReference>,
    /// Exact identity-checked serving certificates compiled on matcher rebuild.
    pub compiled_serving_certificates: Option<Arc<[Arc<CompiledServingCertificate>]>>,
    /// Immutable trust store compiled once when the matcher snapshot is built.
    pub compiled_frontend_ca_store: Option<Arc<X509Store>>,
    /// Immutable frontend client-auth decision compiled when the matcher
    /// generation is published. Reject entries share the same hostname slots,
    /// specificity, and collision precedence as serving entries.
    pub frontend_mtls: FrontendMtlsOutcome,
    /// Session id context of this entry's effective policy, derived once at
    /// construction by `gateway_session_context`. Private so it cannot drift
    /// from the fields it was derived from; read it through `session_context()`.
    session_context: SessionContextId,
}

/// Domain separator for downstream TLS session id contexts.
const SESSION_CONTEXT_DOMAIN: &[u8] = b"edgion.io/downstream-tls-session-context/v1";
pub(crate) const SESSION_CONTEXT_KIND_EDGION_TLS: u8 = 1;
const SESSION_CONTEXT_KIND_GATEWAY: u8 = 2;

/// 32-byte TLS session id context identifying one effective downstream TLS
/// policy (serving identity plus client-authentication requirements).
///
/// BoringSSL resumes a session only when the session's context equals the
/// handshake's, and a resumed handshake neither requests nor re-verifies a
/// client certificate. Every selected policy therefore installs its own id, so
/// a session can never cross from one policy into another on a shared
/// listener. Ids are computed on the cold publication path; the handshake only
/// copies them. The bytes are a digest of public policy data, not a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionContextId([u8; 32]);

impl SessionContextId {
    /// Start a digest for one policy kind under the versioned domain.
    pub(crate) fn hasher(kind: u8) -> FingerprintHasher {
        let mut hasher = FingerprintHasher::new();
        hasher.bytes(SESSION_CONTEXT_DOMAIN);
        hasher.tag(kind);
        hasher
    }

    pub(crate) fn from_digest(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Id for a Gateway frontend-validation decision serving an EdgionTls
    /// certificate overlay: the first half of each input. Both inputs are full
    /// domain-separated SHA-256 digests, so the result differs from either part
    /// and from other pairings with overwhelming probability (2^-128 per pair),
    /// without hashing on the handshake path.
    pub(crate) fn overlay(gateway: &Self, edgion_tls: &Self) -> Self {
        let mut combined = [0u8; 32];
        combined[..16].copy_from_slice(&gateway.0[..16]);
        combined[16..].copy_from_slice(&edgion_tls.0[..16]);
        Self(combined)
    }
}

/// SHA-256 over a length-prefixed DER list (a serving chain or a CA set).
pub(crate) fn der_list_digest<'a>(ders: impl ExactSizeIterator<Item = &'a [u8]>) -> [u8; 32] {
    let mut hasher = FingerprintHasher::new();
    hasher.u64(ders.len() as u64);
    for der in ders {
        hasher.bytes(der);
    }
    hasher.finish()
}

/// Derive a Gateway listener entry's session id context from its effective
/// policy: attachment identity, serving chains, and the frontend client-auth
/// decision with its CA set. The fixed Gateway verify depth is implied by the
/// outcome. `None` serving chains mean compilation failed and the handshake
/// fails closed, so no session can exist under that marker. The matcher always
/// pairs `StrictEffective` with a CA digest (a CA failure compiles to
/// `StrictReject`); the absent-digest marker only keeps the encoding total.
fn gateway_session_context(
    attachment: &ResolvedListenerAttachment,
    serving_chain_digests: Option<&[[u8; 32]]>,
    outcome: FrontendMtlsOutcome,
    frontend_ca_digest: Option<&[u8; 32]>,
) -> SessionContextId {
    let mut hasher = SessionContextId::hasher(SESSION_CONTEXT_KIND_GATEWAY);
    hasher.bytes(attachment.gateway_namespace.as_bytes());
    hasher.bytes(attachment.gateway_name.as_bytes());
    hasher.bytes(attachment.listener_name.as_bytes());
    hasher.bytes(attachment.listener_key.as_ref().as_bytes());
    match serving_chain_digests {
        Some(digests) => {
            hasher.tag(1);
            hasher.u64(digests.len() as u64);
            for digest in digests {
                hasher.bytes(digest);
            }
        }
        None => hasher.tag(0),
    }
    match outcome {
        FrontendMtlsOutcome::NotConfigured => hasher.tag(0),
        FrontendMtlsOutcome::StrictEffective => {
            hasher.tag(1);
            match frontend_ca_digest {
                Some(digest) => {
                    hasher.tag(1);
                    hasher.bytes(digest);
                }
                None => hasher.tag(0),
            }
        }
        FrontendMtlsOutcome::InsecureFallback => hasher.tag(2),
        FrontendMtlsOutcome::StrictReject { .. } => hasher.tag(3),
    }
    SessionContextId(hasher.finish())
}

fn serving_chain_digests(certificates: &Option<Arc<[Arc<CompiledServingCertificate>]>>) -> Option<Vec<[u8; 32]>> {
    certificates.as_ref().map(|certificates| {
        certificates
            .iter()
            .map(|certificate| certificate.chain_digest)
            .collect()
    })
}

impl GatewayTlsEntry {
    /// Session id context installed when this entry is the selected policy.
    pub(crate) fn session_context(&self) -> SessionContextId {
        self.session_context
    }

    /// Test fixture: an entry with no serving certificates or CA store whose
    /// session context is derived exactly as the matcher derives it.
    #[cfg(test)]
    pub(crate) fn for_test(attachment: ResolvedListenerAttachment, frontend_mtls: FrontendMtlsOutcome) -> Self {
        let mut entry = Self {
            attachment,
            gateway_creation_ts: None,
            certificate_refs: Vec::new(),
            compiled_serving_certificates: None,
            compiled_frontend_ca_store: None,
            frontend_mtls,
            session_context: SessionContextId([0; 32]),
        };
        entry.refresh_session_context_for_test(None);
        entry
    }

    /// Re-derive the session context after a fixture mutated public fields.
    #[cfg(test)]
    pub(crate) fn refresh_session_context_for_test(&mut self, frontend_ca_digest: Option<&[u8; 32]>) {
        self.session_context = gateway_session_context(
            &self.attachment,
            serving_chain_digests(&self.compiled_serving_certificates).as_deref(),
            self.frontend_mtls,
            frontend_ca_digest,
        );
    }
}

/// Immutable BoringSSL-ready serving identity compiled while rebuilding the
/// matcher. The chain remains leaf-first and includes all intermediates.
#[derive(Clone)]
pub struct CompiledServingCertificate {
    pub chain: Arc<[X509]>,
    /// SHA-256 over the exact DER chain (leaf first), computed at compile time
    /// for the downstream session id context.
    pub chain_digest: [u8; 32],
    #[cfg(feature = "boringssl")]
    pub atomic_chain: Arc<CompiledBoringCertificateChain>,
    pub private_key: PKey<Private>,
    pub key_type: Id,
    pub validity: LeafValidity,
}

/// Cold-compiled DER buffers consumed by BoringSSL's atomic
/// `SSL_set_chain_and_key`. The function increments the buffers' references on
/// success; this owner keeps the matcher generation's source references alive.
#[cfg(feature = "boringssl")]
pub struct CompiledBoringCertificateChain {
    buffers: Box<[*mut boring_sys::CRYPTO_BUFFER]>,
}

#[cfg(feature = "boringssl")]
unsafe impl Send for CompiledBoringCertificateChain {}
#[cfg(feature = "boringssl")]
unsafe impl Sync for CompiledBoringCertificateChain {}

#[cfg(feature = "boringssl")]
impl CompiledBoringCertificateChain {
    pub(crate) fn from_der(chain: &[Vec<u8>]) -> Option<Self> {
        let mut buffers = Vec::with_capacity(chain.len());
        for der in chain {
            let buffer = unsafe { boring_sys::CRYPTO_BUFFER_new(der.as_ptr(), der.len(), std::ptr::null_mut()) };
            if buffer.is_null() {
                for buffer in buffers {
                    unsafe { boring_sys::CRYPTO_BUFFER_free(buffer) };
                }
                return None;
            }
            buffers.push(buffer);
        }
        Some(Self {
            buffers: buffers.into_boxed_slice(),
        })
    }

    pub fn as_ptr(&self) -> *const *mut boring_sys::CRYPTO_BUFFER {
        self.buffers.as_ptr()
    }

    pub fn len(&self) -> usize {
        self.buffers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }
}

#[cfg(feature = "boringssl")]
impl Drop for CompiledBoringCertificateChain {
    fn drop(&mut self) {
        for buffer in &self.buffers {
            unsafe { boring_sys::CRYPTO_BUFFER_free(*buffer) };
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontendMtlsOutcome {
    NotConfigured,
    StrictEffective,
    StrictReject { reason: &'static str },
    InsecureFallback,
}

#[derive(Clone)]
struct CompiledFrontendMtls {
    outcome: FrontendMtlsOutcome,
    store: Option<Arc<X509Store>>,
    ca_digest: Option<[u8; 32]>,
}

fn compile_frontend_mtls(
    validation: Option<&FrontendTLSValidation>,
    resolved: Option<&[ResolvedFrontendCAReference]>,
    eligibility: Option<FrontendMatcherEligibility>,
    gateway_namespace: &str,
    certificate_ref_count: usize,
    serving_ready: bool,
) -> CompiledFrontendMtls {
    if eligibility == Some(FrontendMatcherEligibility::StrictReject) {
        return compiled_reject("controller rejected gateway tls transport");
    }
    let Some(validation) = validation else {
        return if eligibility != Some(FrontendMatcherEligibility::Programmed)
            || certificate_ref_count == 0
            || !serving_ready
        {
            compiled_reject("frontend projection inconsistent")
        } else {
            CompiledFrontendMtls {
                outcome: FrontendMtlsOutcome::NotConfigured,
                store: None,
                ca_digest: None,
            }
        };
    };
    if validation.ca_certificate_refs.len() > edgion_common::tls_validation::MAX_CA_REFERENCE_COUNT {
        return compiled_reject("ca reference count exceeded");
    }
    match eligibility {
        Some(FrontendMatcherEligibility::StrictReject | FrontendMatcherEligibility::Omit) => {
            return compiled_reject("frontend policy invalid");
        }
        Some(FrontendMatcherEligibility::Programmed) if certificate_ref_count == 0 || !serving_ready => {
            return compiled_reject("serving certificate projection inconsistent");
        }
        Some(FrontendMatcherEligibility::RequireExactEdgionTlsCertificate) if certificate_ref_count != 0 => {
            return compiled_reject("certificate provider projection inconsistent");
        }
        Some(FrontendMatcherEligibility::Programmed | FrontendMatcherEligibility::RequireExactEdgionTlsCertificate) => {
        }
        None => return compiled_reject("frontend eligibility missing"),
    }
    // Only a structurally consistent, admitted projection may select the
    // insecure mode. This prevents a forged StrictReject+insecure pair from
    // bypassing rejection.
    if validation.mode == Some(edgion_resources::resources::gateway::FrontendValidationMode::AllowInsecureFallback) {
        return CompiledFrontendMtls {
            outcome: FrontendMtlsOutcome::InsecureFallback,
            store: None,
            ca_digest: None,
        };
    }
    if validation.ca_certificate_refs.is_empty() {
        return compiled_reject("ca refs empty");
    }
    let Some(resolved) = resolved else {
        return compiled_reject("ca unresolved");
    };
    if resolved.len() != validation.ca_certificate_refs.len() {
        return compiled_reject("ca set incomplete");
    }
    let Ok(mut store_builder) = X509StoreBuilder::new() else {
        return compiled_reject("ca store unavailable");
    };
    let mut aggregate_bytes = 0usize;
    let mut unique_certificates = HashSet::new();
    // Distinct DERs in store-insertion order, hashed once for the session id
    // context after the loop.
    let mut installed_certificates = Vec::new();
    for (index, (reference, outcome)) in validation.ca_certificate_refs.iter().zip(resolved).enumerate() {
        if !reference.group.is_empty() {
            return compiled_reject("ca group invalid");
        }
        let expected_kind = match reference.kind.as_str() {
            "Secret" => FrontendCASourceKind::Secret,
            "ConfigMap" => FrontendCASourceKind::ConfigMap,
            _ => return compiled_reject("ca source invalid"),
        };
        let expected_namespace = reference.namespace.as_deref().unwrap_or(gateway_namespace);
        if outcome.index as usize != index
            || outcome.source.kind != expected_kind
            || outcome.source.name != reference.name
            || outcome.source.namespace != expected_namespace
            || outcome.error.is_some()
        {
            return compiled_reject("ca outcome mismatch");
        }
        let Some(pem) = outcome.ca_certificate.as_ref() else {
            return compiled_reject("ca certificate missing");
        };
        aggregate_bytes = match aggregate_bytes.checked_add(pem.0.len()) {
            Some(total) if total <= edgion_common::tls_validation::MAX_CA_BUNDLE_BYTES_AGGREGATE => total,
            _ => return compiled_reject("ca aggregate bytes exceeded"),
        };
        let Ok(bundle) = edgion_common::tls_validation::parse_canonical_ca_bundle(&pem.0) else {
            return compiled_reject("ca pem invalid");
        };
        for der in bundle.certificates_der {
            if !unique_certificates.insert(der.clone()) {
                continue;
            }
            if unique_certificates.len() > edgion_common::tls_validation::MAX_CA_CERTIFICATES_AGGREGATE {
                return compiled_reject("ca aggregate certificate count exceeded");
            }
            let Ok(certificate) = X509::from_der(&der) else {
                return compiled_reject("ca der invalid");
            };
            if store_builder.add_cert(certificate).is_err() {
                return compiled_reject("ca store invalid");
            }
            installed_certificates.push(der);
        }
    }
    CompiledFrontendMtls {
        outcome: FrontendMtlsOutcome::StrictEffective,
        store: Some(Arc::new(store_builder.build())),
        ca_digest: Some(der_list_digest(installed_certificates.iter().map(Vec::as_slice))),
    }
}

fn compiled_reject(reason: &'static str) -> CompiledFrontendMtls {
    CompiledFrontendMtls {
        outcome: FrontendMtlsOutcome::StrictReject { reason },
        store: None,
        ca_digest: None,
    }
}

fn compile_serving_certificates(
    references: &[SecretObjectReference],
    outcomes: Option<&[ResolvedServingCertificateReference]>,
    gateway_namespace: &str,
) -> Option<Arc<[Arc<CompiledServingCertificate>]>> {
    if references.is_empty() {
        return Some(Arc::from(Vec::new()));
    }
    if references.len() > edgion_common::tls_validation::MAX_SERVING_CERTIFICATE_REFS {
        return None;
    }
    let outcomes = outcomes?;
    if outcomes.len() != references.len() {
        return None;
    }
    let mut certificates = Vec::with_capacity(references.len());
    for (index, (reference, outcome)) in references.iter().zip(outcomes).enumerate() {
        let group = reference.group.as_deref().unwrap_or("");
        let kind = reference.kind.as_deref().unwrap_or("Secret");
        let namespace = reference.namespace.as_deref().unwrap_or(gateway_namespace);
        if !group.is_empty()
            || kind != "Secret"
            || outcome.index as usize != index
            || outcome.source.group != group
            || outcome.source.kind != kind
            || outcome.source.namespace != namespace
            || outcome.source.name != reference.name
            || outcome.error.is_some()
        {
            return None;
        }
        let secret = outcome.secret.as_ref()?;
        if secret.metadata.name.as_deref() != Some(reference.name.as_str())
            || secret.metadata.namespace.as_deref().unwrap_or("default") != namespace
        {
            return None;
        }
        let data = secret.data.as_ref()?;
        let cert = data.get(edgion_resources::constants::secret_keys::tls::CERT)?;
        let key = data.get(edgion_resources::constants::secret_keys::tls::KEY)?;
        let strict_chain = edgion_common::tls_validation::parse_strict_certificate_chain(&cert.0).ok()?;
        edgion_common::tls_validation::parse_strict_private_key_pem(&key.0).ok()?;
        let (cert, key) = (std::str::from_utf8(&cert.0).ok()?, std::str::from_utf8(&key.0).ok()?);
        if edgion_common::tls_validation::check_key_matches_cert(cert, key).is_err() {
            return None;
        }
        #[cfg(feature = "boringssl")]
        let atomic_chain = Arc::new(CompiledBoringCertificateChain::from_der(
            &strict_chain.certificates_der,
        )?);
        let chain_digest = der_list_digest(strict_chain.certificates_der.iter().map(Vec::as_slice));
        let chain = strict_chain
            .certificates_der
            .iter()
            .map(|der| X509::from_der(der).ok())
            .collect::<Option<Vec<_>>>()?;
        let private_key = PKey::private_key_from_pem(key.as_bytes()).ok()?;
        if !chain.first()?.public_key().ok()?.public_eq(&private_key) {
            return None;
        }
        certificates.push(Arc::new(CompiledServingCertificate {
            chain: Arc::from(chain),
            chain_digest,
            #[cfg(feature = "boringssl")]
            atomic_chain,
            key_type: private_key.id(),
            private_key,
            validity: strict_chain.validity,
        }));
    }
    Some(Arc::from(certificates))
}

/// Combined matcher data atomically swapped via `ArcSwap`.
type ListenerMembership = HashMap<String, HashMap<String, HashMap<String, std::collections::HashSet<ListenerKey>>>>;
struct TlsMatcherData {
    eligible_listeners: ListenerMembership,
    /// Hostname-based matcher: listener_key -> (hostname/SNI -> entries)
    listener_map: HashMap<ListenerKey, Rfc6125HostnameMatcher<Vec<Arc<GatewayTlsEntry>>>>,
    /// Catch-all fallback: listener_key -> entries (listeners without hostname)
    fallback_listener_map: HashMap<ListenerKey, Vec<Arc<GatewayTlsEntry>>>,
}

/// Gateway TLS Matcher for listener and hostname-based certificate lookup
///
/// Uses ArcSwap for lock-free reads during TLS handshake.
/// The inner Option provides fast-path for the common case where
/// no Gateway TLS is configured (avoids HashMap allocation and lookup).
///
/// ## Lookup priority (per listener)
/// 1. Exact hostname match (via Rfc6125HostnameMatcher)
/// 2. Wildcard hostname match (via Rfc6125HostnameMatcher)
/// 3. Catch-all fallback (listeners without hostname)
pub struct GatewayTlsMatcher {
    data: ArcSwapOption<TlsMatcherData>,
}

/// One captured native matcher and logical admission generation per handshake.
pub struct GatewayTlsMatcherSnapshot {
    data: Option<Arc<TlsMatcherData>>,
}
impl GatewayTlsMatcherSnapshot {
    pub fn listener_is_eligible(&self, attachment: &ResolvedListenerAttachment) -> bool {
        attachment.gateway_proof.is_resolved()
            && self
                .data
                .as_ref()
                .and_then(|data| data.eligible_listeners.get(attachment.gateway_namespace.as_str()))
                .and_then(|gateways| gateways.get(attachment.gateway_name.as_str()))
                .and_then(|listeners| listeners.get(attachment.listener_name.as_str()))
                .is_some_and(|keys| keys.contains(&attachment.listener_key))
    }
    pub fn match_sni(&self, key: &ListenerKey, sni: &str) -> Option<Arc<GatewayTlsEntry>> {
        let data = self.data.as_ref()?;
        data.listener_map
            .get(key)
            .and_then(|matcher| matcher.get(sni))
            .and_then(|entries| entries.first())
            .cloned()
            .or_else(|| {
                data.fallback_listener_map
                    .get(key)
                    .and_then(|entries| entries.first())
                    .cloned()
            })
    }
}

/// Deterministic ordering for entries that collide in the same matcher slot.
///
/// When two Gateways register TLS for the same hostname (or both as catch-all) on
/// the same physical listener, both entries land in one `Vec`. The winner is the
/// `entries.first()` after this sort, so the order must be stable across rebuilds
/// rather than dependent on upstream HashMap iteration order.
///
/// Tiebreak follows the Gateway API route-conflict convention (oldest
/// `creationTimestamp` wins, then namespace/name), matching
/// `BackendTLSPolicyStore::sort_policies_by_precedence`. `Option<Time>` orders
/// `None` before `Some`, so an entry with a missing timestamp is treated as
/// oldest; `listener_name` is the final disambiguator for two listeners of the
/// same Gateway. This keeps the incumbent Gateway serving when a newer one is
/// added with a colliding hostname.
fn tls_entry_order(a: &Arc<GatewayTlsEntry>, b: &Arc<GatewayTlsEntry>) -> std::cmp::Ordering {
    a.gateway_creation_ts
        .cmp(&b.gateway_creation_ts)
        .then_with(|| a.attachment.gateway_namespace.cmp(&b.attachment.gateway_namespace))
        .then_with(|| a.attachment.gateway_name.cmp(&b.attachment.gateway_name))
        .then_with(|| a.attachment.listener_name.cmp(&b.attachment.listener_name))
}

/// Emit a warning when a TLS matcher slot accumulates more than one entry.
///
/// Runs only on the Controller→Gateway config-sync path (`rebuild_from_gateways`),
/// never on the handshake path, so `tracing` is permitted here. The selected entry
/// is `entries[0]` after `tls_entry_order` sorting.
fn warn_tls_sni_collision(hostname: &str, entries: &[Arc<GatewayTlsEntry>], total: usize) {
    let participants = entries
        .iter()
        .map(|e| {
            format!(
                "{}/{}#{}",
                e.attachment.gateway_namespace, e.attachment.gateway_name, e.attachment.listener_name
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let selected = entries
        .first()
        .map(|e| {
            format!(
                "{}/{}#{}",
                e.attachment.gateway_namespace, e.attachment.gateway_name, e.attachment.listener_name
            )
        })
        .unwrap_or_default();
    tracing::warn!(
        component = "gateway_tls_matcher",
        hostname = %hostname,
        participants = %participants,
        selected = %selected,
        candidate_count = total,
        omitted = total.saturating_sub(entries.len()),
        "Cross-Gateway TLS SNI collision: multiple listeners serve the same hostname on one listener; \
         selecting the certificate deterministically by (creationTimestamp, namespace, name, listener_name)"
    );
}

impl GatewayTlsMatcher {
    pub fn new() -> Self {
        Self {
            data: ArcSwapOption::empty(),
        }
    }

    /// Check if there are any Gateway TLS configurations
    ///
    /// This is a fast O(1) check.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.data
            .load()
            .as_ref()
            .is_none_or(|data| data.listener_map.is_empty() && data.fallback_listener_map.is_empty())
    }

    /// Match SNI against Gateway Listener hostnames with listener key dimension.
    ///
    /// Returns the first matching GatewayTlsEntry when found, else falls through
    /// to `fallback_listener_map` (catch-all listener without a hostname).
    ///
    /// When multiple Gateways register the same hostname on one physical listener
    /// (a cross-Gateway collision the controller's intra-gateway conflict detection
    /// does not block), the slot holds several entries. `rebuild_from_gateways` keeps
    /// each slot sorted by Gateway API precedence (oldest creationTimestamp, then
    /// namespace, name) and logs a warning, so the `entries.first()` selected here is
    /// deterministic across rebuilds rather than dependent on upstream HashMap iteration order.
    ///
    /// Hostname matching follows RFC 6125 §6.4.3 single-label wildcard semantics —
    /// `*.example.com` matches `foo.example.com` but NOT `a.b.example.com`. Route
    /// matching at `TlsRouteTable::match_route` keeps multi-label semantics per
    /// Gateway API §4.3 — the two layers diverge deliberately.
    ///
    /// ## Parameters
    /// - `key`: The listener key identifying the physical listener
    /// - `sni`: Server Name Indication from TLS Client Hello
    ///
    #[inline]
    pub fn match_sni(&self, key: &ListenerKey, sni: &str) -> Option<Arc<GatewayTlsEntry>> {
        self.snapshot().match_sni(key, sni)
    }
    pub fn snapshot(&self) -> GatewayTlsMatcherSnapshot {
        GatewayTlsMatcherSnapshot {
            data: self.data.load_full(),
        }
    }

    /// Rebuild matcher from Gateway list with listener key dimension
    ///
    /// Extracts TLS configurations from all Gateway Listeners and builds
    /// a listener_key -> hostname-based matcher for certificate lookup.
    pub fn rebuild_from_gateways(&self, gateways: &[Gateway]) {
        let mut listener_map: HashMap<ListenerKey, Rfc6125HostnameMatcher<Vec<Arc<GatewayTlsEntry>>>> = HashMap::new();
        let mut fallback_listener_map: HashMap<ListenerKey, Vec<Arc<GatewayTlsEntry>>> = HashMap::new();
        let mut eligible_listeners = ListenerMembership::new();
        let mut hostname_entry_count = 0usize;
        let mut fallback_entry_count = 0usize;
        let mut listener_count = 0usize;

        for gateway in gateways {
            let gateway_namespace = gateway.namespace().unwrap_or_else(|| "default".to_string());
            let gateway_name = gateway.name_any();

            // Get listeners from Gateway spec
            let Some(listeners) = &gateway.spec.listeners else {
                continue;
            };
            if listeners.len() > edgion_resources::resources::gateway::MAX_GATEWAY_LISTENERS {
                tracing::warn!(
                    component = "gateway_tls_matcher",
                    gateway_namespace = %gateway_namespace,
                    gateway_name = %gateway_name,
                    listener_count = listeners.len(),
                    max = edgion_resources::resources::gateway::MAX_GATEWAY_LISTENERS,
                    "Skipping Gateway whose listener count exceeds the runtime limit"
                );
                continue;
            }
            let gateway_proof = gateway_attachment_proof(gateway);

            for listener in listeners {
                if matches!(listener.protocol.as_str(), "HTTPS" | "TLS")
                    && !edgion_resources::resources::gateway::gateway_listener_is_explicitly_ineligible(
                        gateway,
                        &listener.name,
                    )
                {
                    eligible_listeners
                        .entry(gateway_namespace.clone())
                        .or_default()
                        .entry(gateway_name.clone())
                        .or_default()
                        .entry(listener.name.clone())
                        .or_default()
                        .insert(ListenerKey::new(
                            resolve_listener_addr(gateway, listener.port),
                            &listener.protocol,
                        ));
                }
                // Skip listeners without TLS config
                let Some(tls_config) = &listener.tls else {
                    continue;
                };

                let eligibility = tls_config
                    .frontend_matcher_eligibility
                    .unwrap_or(FrontendMatcherEligibility::Omit);
                let shared_rejected = edgion_resources::resources::gateway::gateway_listener_has_shared_policy_conflict(
                    gateway,
                    &listener.name,
                );
                if shared_rejected && eligibility != FrontendMatcherEligibility::StrictReject {
                    continue;
                }
                if eligibility == FrontendMatcherEligibility::Omit {
                    continue;
                }

                // For listeners using EdgionTls cert-provider, certificate_refs may be
                // absent because certs are loaded dynamically via the EdgionTls CRD.
                // We still need to register these listeners so that TLS termination
                // and route matching work on the correct port.
                let has_edgion_tls_provider = tls_config
                    .options
                    .as_ref()
                    .and_then(|opts| opts.get("edgion.io/cert-provider"))
                    .is_some_and(|v| v.as_str() == Some("EdgionTls"));

                let oversized_serving_refs = tls_config
                    .certificate_refs
                    .as_ref()
                    .is_some_and(|refs| refs.len() > edgion_common::tls_validation::MAX_SERVING_CERTIFICATE_REFS);
                let cert_refs = match &tls_config.certificate_refs {
                    // Fail before cloning or traversing attacker-controlled input.
                    Some(refs) if refs.len() > edgion_common::tls_validation::MAX_SERVING_CERTIFICATE_REFS => {
                        Vec::new()
                    }
                    Some(refs) if !refs.is_empty() => refs.clone(),
                    _ if has_edgion_tls_provider
                        || tls_config.frontend_validation.is_some()
                        || tls_config.frontend_matcher_eligibility.is_some() =>
                    {
                        Vec::new()
                    }
                    _ => continue,
                };

                let addr = resolve_listener_addr(gateway, listener.port);
                let listener_key = ListenerKey::new(addr, &listener.protocol);
                let compiled_serving = compile_serving_certificates(
                    &cert_refs,
                    tls_config.resolved_certificate_refs.as_deref(),
                    &gateway_namespace,
                );
                let compiled_frontend = compile_frontend_mtls(
                    tls_config.frontend_validation.as_ref(),
                    tls_config.resolved_frontend_ca_refs.as_deref(),
                    if oversized_serving_refs {
                        Some(FrontendMatcherEligibility::StrictReject)
                    } else {
                        tls_config.frontend_matcher_eligibility
                    },
                    &gateway_namespace,
                    cert_refs.len(),
                    compiled_serving.is_some(),
                );

                let attachment = ResolvedListenerAttachment {
                    gateway_namespace: gateway_namespace.clone(),
                    gateway_name: gateway_name.clone(),
                    listener_name: listener.name.clone(),
                    listener_key: listener_key.clone(),
                    gateway_proof: gateway_proof.clone(),
                };
                let session_context = gateway_session_context(
                    &attachment,
                    serving_chain_digests(&compiled_serving).as_deref(),
                    compiled_frontend.outcome,
                    compiled_frontend.ca_digest.as_ref(),
                );
                let entry = Arc::new(GatewayTlsEntry {
                    attachment,
                    gateway_creation_ts: gateway.metadata.creation_timestamp.clone(),
                    certificate_refs: cert_refs,
                    compiled_serving_certificates: compiled_serving,
                    compiled_frontend_ca_store: compiled_frontend.store,
                    frontend_mtls: compiled_frontend.outcome,
                    session_context,
                });

                match &listener.hostname {
                    Some(hostname) => {
                        let host_matcher = listener_map.entry(listener_key).or_insert_with(|| {
                            listener_count += 1;
                            Rfc6125HostnameMatcher::new()
                        });

                        if let Some(existing) = host_matcher.get_mut_exact(hostname) {
                            // Cross-Gateway collision: another listener already serves this same
                            // hostname pattern (exact or wildcard) on the same physical listener.
                            // The controller's conflict detection is intra-gateway only, so this
                            // case is never admission-blocked.
                            // Keep the slot sorted by Gateway API precedence (oldest
                            // creationTimestamp, then namespace, name, listener_name) so
                            // match_sni's entries.first() is deterministic across rebuilds
                            // instead of following upstream HashMap iteration order.
                            existing.push(entry);
                            existing.sort_by(tls_entry_order);
                            let total = existing.len();
                            warn_tls_sni_collision(
                                hostname,
                                &existing[..existing.len().min(MAX_COLLISION_PARTICIPANTS)],
                                total,
                            );
                        } else {
                            host_matcher.insert(hostname, vec![entry]);
                        }
                        hostname_entry_count += 1;
                    }
                    None => {
                        // Catch-all listener (no hostname) — register as fallback for this listener
                        let slot = fallback_listener_map.entry(listener_key).or_default();
                        slot.push(entry);
                        if slot.len() > 1 {
                            // Multiple catch-all listeners (from different Gateways) on the same
                            // physical listener — same non-deterministic-winner problem as the
                            // hostname branch above. Sort for a deterministic fallback cert.
                            slot.sort_by(tls_entry_order);
                            let total = slot.len();
                            warn_tls_sni_collision(
                                "(catch-all)",
                                &slot[..slot.len().min(MAX_COLLISION_PARTICIPANTS)],
                                total,
                            );
                        }
                        fallback_entry_count += 1;
                    }
                }
            }
        }

        let total_entries = hostname_entry_count + fallback_entry_count;

        tracing::info!(
            component = "gateway_tls_matcher",
            gateways = gateways.len(),
            listeners = listener_count,
            hostname_entries = hostname_entry_count,
            fallback_entries = fallback_entry_count,
            "Rebuilt Gateway TLS matcher"
        );

        if total_entries > 0 || !eligible_listeners.is_empty() {
            self.data.store(Some(Arc::new(TlsMatcherData {
                eligible_listeners,
                listener_map,
                fallback_listener_map,
            })));
        } else {
            self.data.store(None);
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GatewayTlsMatcherStats {
    pub eligible_listener_count: usize,
    pub eligible_listener_keys: Vec<String>,
    pub listener_count: usize,
    pub is_empty: bool,
}

impl GatewayTlsMatcher {
    /// Collect size statistics for leak-detection tests.
    pub fn stats(&self) -> GatewayTlsMatcherStats {
        let data = self.data.load();
        match data.as_deref() {
            Some(d) => {
                let mut all_keys: std::collections::HashSet<&ListenerKey> = d.listener_map.keys().collect();
                all_keys.extend(d.fallback_listener_map.keys());
                let mut eligible_listener_keys = d
                    .eligible_listeners
                    .iter()
                    .flat_map(|(namespace, gateways)| {
                        gateways.iter().flat_map(move |(gateway, listeners)| {
                            listeners.iter().flat_map(move |(listener, keys)| {
                                keys.iter()
                                    .map(move |key| format!("{namespace}/{gateway}/{listener}/{key}"))
                            })
                        })
                    })
                    .collect::<Vec<_>>();
                eligible_listener_keys.sort();
                GatewayTlsMatcherStats {
                    eligible_listener_count: eligible_listener_keys.len(),
                    eligible_listener_keys,
                    listener_count: all_keys.len(),
                    is_empty: all_keys.is_empty(),
                }
            }
            None => GatewayTlsMatcherStats {
                eligible_listener_count: 0,
                eligible_listener_keys: Vec::new(),
                listener_count: 0,
                is_empty: true,
            },
        }
    }
}

impl Default for GatewayTlsMatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Global Gateway TLS Matcher instance
pub static GATEWAY_TLS_MATCHER: LazyLock<GatewayTlsMatcher> = LazyLock::new(GatewayTlsMatcher::new);

/// Get a reference to the global Gateway TLS Matcher
pub fn get_gateway_tls_matcher() -> &'static GatewayTlsMatcher {
    &GATEWAY_TLS_MATCHER
}

/// Match SNI against Gateway TLS configurations with listener key
///
/// This is the preferred method when listener key is known (during TLS handshake).
pub fn match_gateway_tls(key: &ListenerKey, sni: &str) -> Option<Arc<GatewayTlsEntry>> {
    get_gateway_tls_matcher().match_sni(key, sni)
}

/// Rebuild Gateway TLS matcher from Gateway list
pub fn rebuild_gateway_tls_matcher(gateways: &[Gateway]) {
    get_gateway_tls_matcher().rebuild_from_gateways(gateways);
}

// Unit tests for this module live in `tests/tls_test.rs`.
#[cfg(test)]
#[path = "../../../tests/unit/runtime/matching/tls_test.rs"]
mod tests;
