//! Resolver precedence selection.
//!
//! Pure functions that compute an effective [`ResolverSpec`] for a Service from
//! layered configuration (Service annotation → Gateway annotation → EGC default
//! → system, for the data plane; Service annotation → Controller default, for
//! the control plane). No I/O and no runtime resolution — just precedence.
//!
//! The caller assembles an ordered list of per-layer [`ResolverCandidate`]s
//! (most-specific first) and passes a lookup closure that resolves an httpdns
//! LinkSys reference to its config; [`resolve_effective_spec`] returns the
//! winning [`ResolverSpec`].

use std::collections::BTreeMap;

use super::PreparedHttpDns;
use edgion_resources::constants::annotations::edgion::{DNS_RESOLVER, DNS_RESOLVER_REF};
use edgion_resources::resources::edgion_gateway_config::DnsResolverConfig as EgcDnsResolver;
use std::sync::Arc;

use super::ResolverSpec;

/// A parsed httpdns LinkSys reference (`namespace/name`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSysKey {
    pub namespace: String,
    pub name: String,
}

impl LinkSysKey {
    /// Parse a `"namespace/name"` reference. An explicit namespace is required
    /// (bare names are rejected — cross-namespace refs must be unambiguous), and
    /// both halves must be non-empty. Returns `None` for anything malformed.
    pub fn parse(reference: &str) -> Option<Self> {
        let (ns, name) = reference.trim().split_once('/')?;
        let (ns, name) = (ns.trim(), name.trim());
        if ns.is_empty() || name.is_empty() || name.contains('/') {
            return None;
        }
        Some(Self {
            namespace: ns.to_string(),
            name: name.to_string(),
        })
    }
}

/// One layer's resolver choice, before an httpdns ref is resolved to a config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolverCandidate {
    /// Inline standard-DNS server list (`ip[:port]`).
    Servers(Vec<String>),
    /// httpdns LinkSys reference.
    HttpDnsRef(LinkSysKey),
}

/// Split an inline DNS server list (`edgion.io/dns-resolver` value).
///
/// Splits on comma / whitespace, trims, drops blanks, and de-duplicates while
/// preserving order. NOTE: this deliberately does NOT reuse
/// `annotations::edgion::parse_plugin_ref_list` — that helper expands every
/// bare token (no `/`) into `"<default_ns>/<token>"`, which would corrupt a
/// bare IP like `1.1.1.1` into `default/1.1.1.1`.
pub fn parse_dns_server_list(value: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tok in value.split([',', ' ', '\t', '\n']) {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        if !out.iter().any(|s| s == tok) {
            out.push(tok.to_string());
        }
    }
    out
}

/// Extract a resolver candidate from a resource's annotations
/// (`edgion.io/dns-resolver` / `edgion.io/dns-resolver-ref`).
///
/// The two keys are mutually exclusive at ingest, but if both are somehow
/// present the ref wins (an explicit LinkSys reference is the more deliberate
/// choice). Returns `None` when neither key yields a usable value.
pub fn candidate_from_annotations(annotations: &BTreeMap<String, String>) -> Option<ResolverCandidate> {
    if let Some(reference) = annotations.get(DNS_RESOLVER_REF) {
        if let Some(key) = LinkSysKey::parse(reference) {
            return Some(ResolverCandidate::HttpDnsRef(key));
        }
    }
    if let Some(servers) = annotations.get(DNS_RESOLVER) {
        let list = parse_dns_server_list(servers);
        if !list.is_empty() {
            return Some(ResolverCandidate::Servers(list));
        }
    }
    None
}

/// Extract a resolver candidate from the EdgionGatewayConfig data-plane default.
/// `linkSysRef` wins over `servers` if both are set (they are mutually exclusive
/// at ingest).
pub fn candidate_from_egc_default(egc: &EgcDnsResolver) -> Option<ResolverCandidate> {
    if let Some(reference) = &egc.link_sys_ref {
        return Some(ResolverCandidate::HttpDnsRef(LinkSysKey {
            namespace: reference.namespace.clone(),
            name: reference.name.clone(),
        }));
    }
    if let Some(servers) = &egc.servers {
        if !servers.is_empty() {
            return Some(ResolverCandidate::Servers(servers.clone()));
        }
    }
    None
}

/// Resolve the effective [`ResolverSpec`] from ordered candidate layers
/// (most-specific first). The first layer that carries a candidate wins.
///
/// - `Servers(non-empty)` → [`ResolverSpec::StandardDns`].
/// - `Servers(empty)` → [`ResolverSpec::System`] (an empty list is not a
///   resolver; fall back rather than build an unusable standard-DNS spec).
/// - `HttpDnsRef` → resolved via `lookup`; a dangling ref (not found / not an
///   httpdns LinkSys) fails safe to [`ResolverSpec::System`] rather than
///   silently dropping to a lower-precedence layer.
/// - No candidate in any layer → [`ResolverSpec::System`].
pub fn resolve_effective_spec<F>(layers: &[Option<ResolverCandidate>], lookup: F) -> ResolverSpec
where
    F: Fn(&LinkSysKey) -> Option<Arc<PreparedHttpDns>>,
{
    // The first configured layer (most-specific first) wins; `flatten` skips the
    // unconfigured (`None`) layers.
    match layers.iter().flatten().next() {
        None => ResolverSpec::System,
        Some(ResolverCandidate::Servers(servers)) if !servers.is_empty() => ResolverSpec::StandardDns {
            servers: servers.clone(),
        },
        Some(ResolverCandidate::Servers(_)) => ResolverSpec::System,
        Some(ResolverCandidate::HttpDnsRef(key)) => match lookup(key) {
            Some(prepared) => ResolverSpec::HttpDns(prepared),
            None => ResolverSpec::System,
        },
    }
}

#[cfg(test)]
#[path = "../../tests/unit/dns/resolver_select_test.rs"]
mod tests;
