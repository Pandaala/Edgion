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
use edgion_resources::listener_key::{resolve_listener_addr, ListenerKey};
use edgion_resources::prelude_resources::Gateway;
use edgion_resources::resources::edgion_tls::GatewayAttachmentProof;
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
    /// Gateway namespace
    pub gateway_namespace: String,
    /// Gateway name
    pub gateway_name: String,
    /// Gateway `metadata.creationTimestamp` — primary tiebreak key when two
    /// Gateways collide on the same hostname (oldest wins, per the Gateway API
    /// route-conflict convention; mirrors `BackendTLSPolicyStore` precedence).
    pub gateway_creation_ts: Option<Time>,
    /// Listener name
    pub listener_name: String,
    /// Listener key identifying the physical listener ("{addr}/{protocol}")
    pub listener_key: ListenerKey,
    /// Exact Controller-captured Gateway instance/generation/epoch proof.
    pub gateway_proof: GatewayAttachmentProof,
    /// Hostname pattern (e.g., "*.example.com")
    pub hostname: String,
    /// References to Secrets containing certificates
    pub certificate_refs: Vec<SecretObjectReference>,
    /// Exact identity-checked serving certificates compiled on matcher rebuild.
    pub compiled_serving_certificates: Option<Arc<Vec<Arc<CompiledServingCertificate>>>>,
    /// Immutable trust store compiled once when the matcher snapshot is built.
    pub compiled_frontend_ca_store: Option<Arc<X509Store>>,
    /// Immutable frontend client-auth decision compiled when the matcher
    /// generation is published. Reject entries share the same hostname slots,
    /// specificity, and collision precedence as serving entries.
    pub frontend_mtls: FrontendMtlsOutcome,
}

/// Immutable BoringSSL-ready serving identity compiled while rebuilding the
/// matcher. The chain remains leaf-first and includes all intermediates.
#[derive(Clone)]
pub struct CompiledServingCertificate {
    pub chain: Arc<Vec<X509>>,
    #[cfg(feature = "boringssl")]
    pub atomic_chain: Arc<CompiledBoringCertificateChain>,
    pub private_key: PKey<Private>,
    pub key_type: Id,
    pub leaf_not_before_unix: i64,
    pub leaf_not_after_unix: i64,
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
    let mut store_builder = match X509StoreBuilder::new() {
        Ok(builder) => builder,
        Err(_) => return compiled_reject("ca store unavailable"),
    };
    let mut aggregate_bytes = 0usize;
    let mut unique_certificates = HashSet::new();
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
        let bundle = match edgion_common::tls_validation::parse_canonical_ca_bundle(&pem.0) {
            Ok(bundle) => bundle,
            Err(_) => return compiled_reject("ca pem invalid"),
        };
        for der in bundle.certificates_der {
            if !unique_certificates.insert(der.clone()) {
                continue;
            }
            if unique_certificates.len() > edgion_common::tls_validation::MAX_CA_CERTIFICATES_AGGREGATE {
                return compiled_reject("ca aggregate certificate count exceeded");
            }
            let certificate = match X509::from_der(&der) {
                Ok(certificate) => certificate,
                Err(_) => return compiled_reject("ca der invalid"),
            };
            if store_builder.add_cert(certificate).is_err() {
                return compiled_reject("ca store invalid");
            }
        }
    }
    CompiledFrontendMtls {
        outcome: FrontendMtlsOutcome::StrictEffective,
        store: Some(Arc::new(store_builder.build())),
    }
}

fn compiled_reject(reason: &'static str) -> CompiledFrontendMtls {
    CompiledFrontendMtls {
        outcome: FrontendMtlsOutcome::StrictReject { reason },
        store: None,
    }
}

fn compile_serving_certificates(
    references: &[SecretObjectReference],
    outcomes: Option<&[ResolvedServingCertificateReference]>,
    gateway_namespace: &str,
) -> Option<Arc<Vec<Arc<CompiledServingCertificate>>>> {
    if references.is_empty() {
        return Some(Arc::new(Vec::new()));
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
        if group != ""
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
            || secret.metadata.namespace.as_deref() != Some(namespace)
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
            chain: Arc::new(chain),
            #[cfg(feature = "boringssl")]
            atomic_chain,
            key_type: private_key.id(),
            private_key,
            leaf_not_before_unix: strict_chain.leaf_not_before_unix,
            leaf_not_after_unix: strict_chain.leaf_not_after_unix,
        }));
    }
    Some(Arc::new(certificates))
}

/// Combined matcher data atomically swapped via `ArcSwap`.
struct TlsMatcherData {
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
        .then_with(|| a.gateway_namespace.cmp(&b.gateway_namespace))
        .then_with(|| a.gateway_name.cmp(&b.gateway_name))
        .then_with(|| a.listener_name.cmp(&b.listener_name))
}

/// Emit a warning when a TLS matcher slot accumulates more than one entry.
///
/// Runs only on the Controller→Gateway config-sync path (`rebuild_from_gateways`),
/// never on the handshake path, so `tracing` is permitted here. The selected entry
/// is `entries[0]` after `tls_entry_order` sorting.
fn warn_tls_sni_collision(hostname: &str, entries: &[Arc<GatewayTlsEntry>], total: usize) {
    let participants = entries
        .iter()
        .map(|e| format!("{}/{}#{}", e.gateway_namespace, e.gateway_name, e.listener_name))
        .collect::<Vec<_>>()
        .join(", ");
    let selected = entries
        .first()
        .map(|e| format!("{}/{}#{}", e.gateway_namespace, e.gateway_name, e.listener_name))
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
        self.data.load().is_none()
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
        let data_guard = self.data.load();
        let data = data_guard.as_ref()?;

        // 1) Hostname-based lookup (exact > wildcard)
        if let Some(host_matcher) = data.listener_map.get(key) {
            if let Some(entries) = host_matcher.get(sni) {
                if let Some(entry) = entries.first() {
                    return Some(entry.clone());
                }
            }
        }

        // 2) Catch-all fallback (listeners without hostname)
        if let Some(entries) = data.fallback_listener_map.get(key) {
            if let Some(entry) = entries.first() {
                return Some(entry.clone());
            }
        }

        None
    }

    /// Rebuild matcher from Gateway list with listener key dimension
    ///
    /// Extracts TLS configurations from all Gateway Listeners and builds
    /// a listener_key -> hostname-based matcher for certificate lookup.
    pub fn rebuild_from_gateways(&self, gateways: &[Gateway]) {
        let mut listener_map: HashMap<ListenerKey, Rfc6125HostnameMatcher<Vec<Arc<GatewayTlsEntry>>>> = HashMap::new();
        let mut fallback_listener_map: HashMap<ListenerKey, Vec<Arc<GatewayTlsEntry>>> = HashMap::new();
        let mut hostname_entry_count = 0usize;
        let mut fallback_entry_count = 0usize;
        let mut listener_count = 0usize;

        for gateway in gateways {
            let gateway_namespace = gateway.namespace().unwrap_or_default();
            let gateway_name = gateway.name_any();

            // Get listeners from Gateway spec
            let listeners = match &gateway.spec.listeners {
                Some(listeners) => listeners,
                None => continue,
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

            for listener in listeners {
                // Skip listeners without TLS config
                let tls_config = match &listener.tls {
                    Some(tls) => tls,
                    None => continue,
                };

                let eligibility = tls_config
                    .frontend_matcher_eligibility
                    .unwrap_or(FrontendMatcherEligibility::Omit);
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

                let addr = resolve_listener_addr(gateway.spec.addresses.as_ref(), listener.port);
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

                match &listener.hostname {
                    Some(hostname) => {
                        let entry = Arc::new(GatewayTlsEntry {
                            attachment: ResolvedListenerAttachment {
                                gateway_namespace: gateway_namespace.clone(),
                                gateway_name: gateway_name.clone(),
                                listener_name: listener.name.clone(),
                                listener_key: listener_key.clone(),
                                gateway_proof: gateway_attachment_proof(gateway),
                            },
                            gateway_namespace: gateway_namespace.clone(),
                            gateway_name: gateway_name.clone(),
                            gateway_creation_ts: gateway.metadata.creation_timestamp.clone(),
                            listener_name: listener.name.clone(),
                            listener_key: listener_key.clone(),
                            gateway_proof: gateway_attachment_proof(gateway),
                            hostname: hostname.clone(),
                            certificate_refs: cert_refs,
                            compiled_serving_certificates: compiled_serving.clone(),
                            compiled_frontend_ca_store: compiled_frontend.store.clone(),
                            frontend_mtls: compiled_frontend.outcome,
                        });

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
                        let entry = Arc::new(GatewayTlsEntry {
                            attachment: ResolvedListenerAttachment {
                                gateway_namespace: gateway_namespace.clone(),
                                gateway_name: gateway_name.clone(),
                                listener_name: listener.name.clone(),
                                listener_key: listener_key.clone(),
                                gateway_proof: gateway_attachment_proof(gateway),
                            },
                            gateway_namespace: gateway_namespace.clone(),
                            gateway_name: gateway_name.clone(),
                            gateway_creation_ts: gateway.metadata.creation_timestamp.clone(),
                            listener_name: listener.name.clone(),
                            listener_key: listener_key.clone(),
                            gateway_proof: gateway_attachment_proof(gateway),
                            hostname: String::new(),
                            certificate_refs: cert_refs,
                            compiled_serving_certificates: compiled_serving.clone(),
                            compiled_frontend_ca_store: compiled_frontend.store.clone(),
                            frontend_mtls: compiled_frontend.outcome,
                        });
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

        if total_entries > 0 {
            self.data.store(Some(Arc::new(TlsMatcherData {
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
                GatewayTlsMatcherStats {
                    listener_count: all_keys.len(),
                    is_empty: false,
                }
            }
            None => GatewayTlsMatcherStats {
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
#[path = "tests/tls_test.rs"]
mod tests;
