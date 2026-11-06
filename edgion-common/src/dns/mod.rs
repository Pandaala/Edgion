//! Shared DNS resolver used by both the controller (background resolution)
//! and the gateway (data-plane cache). Backend-agnostic; the standard-DNS
//! backend uses hickory-resolver, HTTP DNS reuses link_sys_core's HTTP client,
//! and the no-config path falls back to the system resolver.

pub mod cache;
mod hickory;
pub mod http_fetcher;
pub mod httpdns;
pub mod resolver_select;
pub mod standard;
pub mod system;

pub use edgion_resources::resources::common::DnsLookupFamily;
use std::borrow::Cow;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

pub use cache::{egress_answer_cache, DnsAnswerCache, DnsAnswerCacheStats, DnsCachePolicy};

/// Explicit query intent: infrastructure preserves its native resolver behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum DnsQueryPolicy {
    Native,
    Family(DnsLookupFamily),
}

impl DnsQueryPolicy {
    pub fn select(self, ips: Vec<IpAddr>) -> Vec<IpAddr> {
        if self == Self::Native {
            return ips;
        }
        let mut out = Vec::new();
        for ip in ips {
            let ip = ip.to_canonical();
            if self.accepts(ip) && !out.contains(&ip) {
                out.push(ip);
            }
        }
        out
    }
    pub fn accepts(self, ip: IpAddr) -> bool {
        match self {
            Self::Native => true,
            Self::Family(family) => family.allows(ip),
        }
    }
}

pub(super) fn require_addresses(ips: Vec<IpAddr>) -> Result<Vec<IpAddr>, DnsError> {
    if ips.is_empty() {
        Err(DnsError::NoAddresses)
    } else {
        Ok(ips)
    }
}

/// A resolver answer: the addresses plus the remaining record lifetime when the
/// resolver reports one (`None` for the System resolver and TTL-less HTTPDNS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsAnswer {
    pub addrs: Vec<IpAddr>,
    pub ttl: Option<Duration>,
}

impl DnsAnswer {
    /// An answer whose lifetime the resolver did not report.
    pub fn without_ttl(addrs: Vec<IpAddr>) -> Self {
        Self { addrs, ttl: None }
    }
}

/// Smallest reported TTL; an unreported TTL does not shorten a reported one.
pub(super) fn min_ttl(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

use crate::link_sys_core::outbound_tls::{
    validate_active_tls_inheritance, validate_local_tls_selection, GlobalOutboundTlsSnapshot, TlsMaterialProofError,
};
use edgion_resources::resources::link_sys::HttpDnsResolverConfig;
use edgion_resources::resources::refs::link_sys_fields::PreparedLinkSysReferences;
use edgion_resources::LinkSys;

/// Configuration-owned HTTP DNS generation. Querying never builds clients or
/// reads inherited TLS policy; an unusable generation retains its own fallback.
pub struct PreparedHttpDns {
    query: httpdns::PreparedQuery,
    config: HttpDnsResolverConfig,
    fetcher: Option<http_fetcher::ReqwestHttpDnsFetcher>,
    address_fingerprint: String,
    runtime_identity: [u8; 32],
}

impl std::fmt::Debug for PreparedHttpDns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedHttpDns")
            .field("ready", &self.is_ready())
            .finish_non_exhaustive()
    }
}

/// Cold actual-owner input shared by reuse comparison and construction.
/// No policy, material, owner identity or provider branch can be replaced.
/// ```compile_fail
/// use std::sync::Arc;
/// use edgion_resources::{LinkSys, resources::refs::link_sys_fields::PreparedLinkSysReferences};
/// use edgion_common::{dns::HttpDnsCandidate, link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot};
/// fn forge(owner: Arc<LinkSys>, references: PreparedLinkSysReferences, defaults: Arc<GlobalOutboundTlsSnapshot>) -> HttpDnsCandidate {
///     HttpDnsCandidate { owner, references, defaults, authority: Ok(()), runtime_identity: [0; 32] }
/// }
/// ```
/// A detached provider config cannot supply owning provenance.
/// ```compile_fail
/// use std::sync::Arc;
/// use edgion_resources::resources::link_sys::HttpDnsResolverConfig;
/// use edgion_common::dns::HttpDnsCandidate;
/// fn detached(config: HttpDnsResolverConfig) {
///     HttpDnsCandidate::from_link_sys(Arc::new(config));
/// }
/// ```
pub struct HttpDnsCandidate {
    owner: Arc<LinkSys>,
    references: PreparedLinkSysReferences,
    defaults: Arc<GlobalOutboundTlsSnapshot>,
    authority: Result<(), TlsMaterialProofError>,
    runtime_identity: [u8; 32],
}

impl std::fmt::Debug for HttpDnsCandidate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("HttpDnsCandidate").finish_non_exhaustive()
    }
}
impl PreparedHttpDns {
    pub fn is_ready(&self) -> bool {
        self.fetcher.is_some()
    }
}
impl HttpDnsCandidate {
    /// Only another provider is absent. Invalid HTTPDNS retains its fallback.
    pub fn from_link_sys(owner: Arc<LinkSys>) -> Option<Self> {
        Self::with_snapshot(owner, crate::link_sys_core::outbound_tls::outbound_global_default())
    }

    pub fn with_snapshot(owner: Arc<LinkSys>, defaults: Arc<GlobalOutboundTlsSnapshot>) -> Option<Self> {
        let config = owner.spec.config.as_http_dns()?;
        let references = PreparedLinkSysReferences::from_link_sys(&owner);
        let authority = {
            references
                .bind(&owner)
                .map_err(TlsMaterialProofError::Reference)
                .and_then(|binding| {
                    let selection = binding
                        .outbound_tls_selection()
                        .map_err(TlsMaterialProofError::Reference)?;
                    let proof = validate_local_tls_selection(selection)?;
                    validate_active_tls_inheritance(&proof, &defaults)
                })
        };
        let runtime_identity = Self::identity_for(config, &defaults, authority);
        Some(Self {
            owner,
            references,
            defaults,
            authority,
            runtime_identity,
        })
    }

    /// Pure incoming-authority gate before material/equality replay shortcuts.
    pub fn matches(&self, previous: &PreparedHttpDns) -> bool {
        if previous.is_ready() && self.authority.is_err() {
            return false;
        }
        // Reuse a matching failed outcome without rebuilding/auditing each tick.
        self.runtime_identity == previous.runtime_identity
    }

    /// Even failed preparation publishes the actual configuration's fallback.
    pub fn prepare(self) -> Arc<PreparedHttpDns> {
        let config = self.config();
        let fetcher = self.build_fetcher().ok();
        // Readiness is part of the address identity: a fallback-only generation
        // must never share cached answers with a ready one.
        let address_fingerprint = {
            let _redact = edgion_resources::redact::AdminRedactGuard::new();
            let readiness = if fetcher.is_some() { "ready" } else { "fallback" };
            format!(
                "httpdns:{readiness}:{}",
                serde_json::to_string(config).unwrap_or_default()
            )
        };
        // Clone only query policy: never copy connection/Secret bodies to discard.
        let query = HttpDnsResolverConfig {
            preset: config.preset.clone(),
            url_template: config.url_template.clone(),
            response: config.response.clone(),
            fallback: config.fallback.clone(),
            connection: None,
        };
        Arc::new(PreparedHttpDns {
            query: httpdns::PreparedQuery::from_config(config),
            config: query,
            fetcher,
            address_fingerprint,
            runtime_identity: self.runtime_identity,
        })
    }

    fn config(&self) -> &HttpDnsResolverConfig {
        self.owner
            .spec
            .config
            .as_http_dns()
            .expect("private HTTPDNS candidate retains its fixed provider")
    }

    fn build_fetcher(&self) -> Result<http_fetcher::ReqwestHttpDnsFetcher, DnsError> {
        let config = self.config();
        if !config.validate().is_empty() {
            return Err(DnsError::Http("invalid HTTP DNS configuration".into()));
        }
        let binding = self
            .references
            .bind(&self.owner)
            .map_err(|_| DnsError::Http("invalid HTTP DNS owning reference".into()))?;
        let selection = binding
            .outbound_tls_selection()
            .map_err(|_| DnsError::Http("invalid HTTP DNS owning reference".into()))?;
        let proof = validate_local_tls_selection(selection)
            .map_err(|_| DnsError::Http("invalid HTTP DNS TLS reference or material".into()))?;
        http_fetcher::ReqwestHttpDnsFetcher::from_bound(config.connection.as_ref(), &proof, &self.defaults)
    }

    fn identity_for(
        config: &HttpDnsResolverConfig,
        defaults: &GlobalOutboundTlsSnapshot,
        authority: Result<(), TlsMaterialProofError>,
    ) -> [u8; 32] {
        use crate::link_sys_core::fingerprint::{hash_tls_inheritance, hash_tls_material, FingerprintHasher};
        let mut hash = FingerprintHasher::new();
        match authority {
            Ok(()) => hash.tag(0),
            Err(error) => {
                hash.tag(1);
                // Fixed diagnostics carry neither authored identity nor material.
                hash.bytes(error.to_string().as_bytes());
            }
        }
        {
            let _redact = edgion_resources::redact::AdminRedactGuard::new();
            hash.bytes(&serde_json::to_vec(config).unwrap_or_default());
        }
        let local = config
            .connection
            .as_ref()
            .and_then(|connection| connection.tls.as_ref());
        hash_tls_material(&mut hash, local);
        hash_tls_inheritance(
            &mut hash,
            local.filter(|tls| tls.enabled).map(|tls| &tls.outbound),
            defaults,
        );
        hash.finish()
    }
}

/// Strip a single pair of surrounding `[` `]` brackets, if present (bracketed
/// IPv6 literal without a port, e.g. `"[::1]"`). Shared by the standard-DNS
/// server-string parser and the HTTP DNS response-body IP parser so a
/// bracketed IPv6 literal parses the same way in both places.
pub(crate) fn strip_brackets(s: &str) -> &str {
    s.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(s)
}

/// A configuration-prepared standard-DNS server list with its precomputed
/// cache identity, shared by `Arc` so per-request resolver selection never
/// clones the list or rebuilds the fingerprint.
#[derive(Debug, PartialEq, Eq)]
pub struct StandardDnsServers {
    servers: Vec<String>,
    fingerprint: String,
}

impl StandardDnsServers {
    pub fn new(servers: Vec<String>) -> Arc<Self> {
        let fingerprint = format!("std:{}", servers.join(","));
        Arc::new(Self { servers, fingerprint })
    }

    pub fn servers(&self) -> &[String] {
        &self.servers
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }
}

/// A fully-resolved resolver choice (precedence resolution happens in callers).
#[derive(Debug, Clone)]
pub enum ResolverSpec {
    /// OS/system resolver (no configuration).
    System,
    /// Standard DNS against an explicit server list (`ip[:port]`).
    StandardDns(Arc<StandardDnsServers>),
    /// HTTP DNS via a configuration-prepared LinkSys generation.
    HttpDns(Arc<PreparedHttpDns>),
}

impl ResolverSpec {
    /// A stable fingerprint of this resolver choice, used as a data-plane cache
    /// key component so a resolver change (different servers, a swapped httpdns
    /// config) invalidates cached addresses instead of serving stale ones.
    ///
    /// HTTP DNS returns a precomputed admin-redacted address-cache identity
    /// that includes the generation's readiness. Secret bytes belong only to
    /// the private preparation identity; rotating them does not invalidate
    /// otherwise unchanged resolved backend addresses.
    ///
    /// Every variant borrows a precomputed value, so cache lookups allocate
    /// nothing for the resolver identity.
    pub fn fingerprint(&self) -> Cow<'_, str> {
        match self {
            ResolverSpec::System => Cow::Borrowed("system"),
            ResolverSpec::StandardDns(servers) => Cow::Borrowed(&servers.fingerprint),
            ResolverSpec::HttpDns(prepared) => Cow::Borrowed(&prepared.address_fingerprint),
        }
    }
}

/// Resolver failure classes (stable, low-cardinality).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsError {
    Io(String),
    Http(String),
    Parse(String),
    NoAddresses,
}

impl std::fmt::Display for DnsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DnsError::Io(s) => write!(f, "dns io error: {s}"),
            DnsError::Http(s) => write!(f, "http dns error: {s}"),
            DnsError::Parse(s) => write!(f, "http dns parse error: {s}"),
            DnsError::NoAddresses => write!(f, "no addresses resolved"),
        }
    }
}
impl std::error::Error for DnsError {}

/// Resolve `domain` per `spec`. Returns all resolved addresses.
pub async fn resolve(spec: &ResolverSpec, domain: &str, policy: DnsQueryPolicy) -> Result<Vec<IpAddr>, DnsError> {
    resolve_answer(spec, domain, policy).await.map(|answer| answer.addrs)
}

/// Resolve `domain` per `spec`, returning the addresses and the remaining
/// record lifetime when the resolver reports one. Uncached; callers that want
/// caching go through [`DnsAnswerCache`].
pub async fn resolve_answer(spec: &ResolverSpec, domain: &str, policy: DnsQueryPolicy) -> Result<DnsAnswer, DnsError> {
    if policy != DnsQueryPolicy::Native {
        if let Ok(ip) = strip_brackets(domain).parse::<IpAddr>() {
            return require_addresses(policy.select(vec![ip])).map(DnsAnswer::without_ttl);
        }
    }
    match spec {
        ResolverSpec::System => {
            let addrs = system::resolve_system(domain, policy)
                .await
                .map_err(|e| DnsError::Io(e.to_string()))?;
            require_addresses(addrs).map(DnsAnswer::without_ttl)
        }
        ResolverSpec::StandardDns(servers) => standard::resolve_standard(servers.servers(), domain, policy).await,
        ResolverSpec::HttpDns(prepared) => match prepared.fetcher.as_ref() {
            Some(fetcher) => {
                httpdns::resolve_http_dns_prepared(fetcher, &prepared.config, &prepared.query, domain, policy).await
            }
            None => httpdns::resolve_via_fallback(&prepared.config, domain, policy).await,
        },
    }
}

#[cfg(test)]
#[path = "../../tests/unit/dns/mod_test.rs"]
mod tests;
