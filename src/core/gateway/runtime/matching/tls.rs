//! Gateway TLS Matcher
//!
//! Provides fallback TLS certificate lookup based on Gateway Listener configurations.
//! This matcher is used when EdgionTls lookup fails.
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

use crate::core::common::matcher::HashHost;
use crate::core::gateway::runtime::store::listener_gateway_info::is_listener_not_programmed;
use crate::types::listener_key::{resolve_listener_addr, ListenerKey};
use crate::types::prelude_resources::Gateway;
use crate::types::resources::gateway::{FrontendTLSValidation, SecretObjectReference};
use arc_swap::ArcSwapOption;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use kube::ResourceExt;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

/// Gateway TLS entry containing certificate references and resolved secrets
#[derive(Debug, Clone)]
pub struct GatewayTlsEntry {
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
    /// Hostname pattern (e.g., "*.example.com")
    pub hostname: String,
    /// References to Secrets containing certificates
    pub certificate_refs: Vec<SecretObjectReference>,
    /// Resolved Secret data (filled by Controller)
    pub secrets: Option<Vec<Secret>>,
    /// Frontend validation configuration (Gateway API standard mTLS)
    pub frontend_validation: Option<FrontendTLSValidation>,
    /// Resolved CA Secrets for frontend validation
    pub resolved_frontend_ca_secrets: Option<Vec<Secret>>,
}

/// Combined matcher data atomically swapped via `ArcSwap`.
struct TlsMatcherData {
    /// Hostname-based matcher: listener_key -> (hostname/SNI -> entries)
    listener_map: HashMap<ListenerKey, HashHost<Vec<GatewayTlsEntry>>>,
    /// Catch-all fallback: listener_key -> entries (listeners without hostname)
    fallback_listener_map: HashMap<ListenerKey, Vec<GatewayTlsEntry>>,
}

/// Gateway TLS Matcher for listener and hostname-based certificate lookup
///
/// Uses ArcSwap for lock-free reads during TLS handshake.
/// The inner Option provides fast-path for the common case where
/// no Gateway TLS is configured (avoids HashMap allocation and lookup).
///
/// ## Lookup priority (per listener)
/// 1. Exact hostname match (via HashHost)
/// 2. Wildcard hostname match (via HashHost)
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
fn tls_entry_order(a: &GatewayTlsEntry, b: &GatewayTlsEntry) -> std::cmp::Ordering {
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
fn warn_tls_sni_collision(hostname: &str, entries: &[GatewayTlsEntry]) {
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
    pub fn match_sni(&self, key: &ListenerKey, sni: &str) -> Option<GatewayTlsEntry> {
        let data_guard = self.data.load();
        let data = data_guard.as_ref()?;

        // 1) Hostname-based lookup (exact > wildcard)
        if let Some(host_matcher) = data.listener_map.get(key) {
            if let Some(entries) = host_matcher.get_rfc6125(sni) {
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
        let mut listener_map: HashMap<ListenerKey, HashHost<Vec<GatewayTlsEntry>>> = HashMap::new();
        let mut fallback_listener_map: HashMap<ListenerKey, Vec<GatewayTlsEntry>> = HashMap::new();
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

            for listener in listeners {
                // Skip listeners without TLS config
                let tls_config = match &listener.tls {
                    Some(tls) => tls,
                    None => continue,
                };

                // Skip listeners the Controller has marked as not programmed (e.g. invalid CA cert
                // reference, unresolvable Secret). Registering them would allow a broken TLS config
                // to serve or shadow valid listeners on the same port.
                if is_listener_not_programmed(gateway, &listener.name) {
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

                let cert_refs = match &tls_config.certificate_refs {
                    Some(refs) if !refs.is_empty() => refs.clone(),
                    _ if has_edgion_tls_provider => Vec::new(),
                    _ => continue,
                };

                let addr = resolve_listener_addr(gateway.spec.addresses.as_ref(), listener.port);
                let listener_key = ListenerKey::new(addr, &listener.protocol);

                match &listener.hostname {
                    Some(hostname) => {
                        let entry = GatewayTlsEntry {
                            gateway_namespace: gateway_namespace.clone(),
                            gateway_name: gateway_name.clone(),
                            gateway_creation_ts: gateway.metadata.creation_timestamp.clone(),
                            listener_name: listener.name.clone(),
                            listener_key: listener_key.clone(),
                            hostname: hostname.clone(),
                            certificate_refs: cert_refs,
                            secrets: tls_config.secrets.clone(),
                            frontend_validation: tls_config.frontend_validation.clone(),
                            resolved_frontend_ca_secrets: tls_config.resolved_frontend_ca_secrets.clone(),
                        };

                        let host_matcher = listener_map.entry(listener_key).or_insert_with(|| {
                            listener_count += 1;
                            HashHost::new()
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
                            warn_tls_sni_collision(hostname, existing);
                        } else {
                            host_matcher.insert(hostname, vec![entry]);
                        }
                        hostname_entry_count += 1;
                    }
                    None => {
                        // Catch-all listener (no hostname) — register as fallback for this listener
                        let entry = GatewayTlsEntry {
                            gateway_namespace: gateway_namespace.clone(),
                            gateway_name: gateway_name.clone(),
                            gateway_creation_ts: gateway.metadata.creation_timestamp.clone(),
                            listener_name: listener.name.clone(),
                            listener_key: listener_key.clone(),
                            hostname: String::new(),
                            certificate_refs: cert_refs,
                            secrets: tls_config.secrets.clone(),
                            frontend_validation: tls_config.frontend_validation.clone(),
                            resolved_frontend_ca_secrets: tls_config.resolved_frontend_ca_secrets.clone(),
                        };
                        let slot = fallback_listener_map.entry(listener_key).or_default();
                        slot.push(entry);
                        if slot.len() > 1 {
                            // Multiple catch-all listeners (from different Gateways) on the same
                            // physical listener — same non-deterministic-winner problem as the
                            // hostname branch above. Sort for a deterministic fallback cert.
                            slot.sort_by(tls_entry_order);
                            warn_tls_sni_collision("(catch-all)", slot);
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
pub fn match_gateway_tls(key: &ListenerKey, sni: &str) -> Option<GatewayTlsEntry> {
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
