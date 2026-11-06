//! HTTP DNS backend: render a request URL, fetch it, parse the response into
//! IPs, and apply a configured fallback on failure.

use std::collections::HashSet;
use std::net::IpAddr;

use edgion_resources::resources::link_sys::httpdns::{
    HttpDnsBodyKind, HttpDnsFallback, HttpDnsResolverConfig, HttpDnsResponseFormat,
};

use super::{
    min_ttl, require_addresses, standard, strip_brackets, system, DnsAnswer, DnsError, DnsLookupFamily, DnsQueryPolicy,
};
use std::time::Duration;

/// Hard cap on unique HTTP DNS addresses. Native queries retain their historical
/// prefix truncation; family-aware queries reject overflow before filtering, so
/// a truncated family can never be interpreted as an authoritative removal.
const MAX_HTTP_DNS_ADDRS: usize = 256;

/// Isolates the network fetch so rendering/parsing/fallback stay unit-testable.
#[async_trait::async_trait]
pub trait HttpDnsFetcher {
    async fn get(&self, url: &str) -> Result<String, DnsError>;
}

/// Hostname charset accepted in a domain spliced into a URL template:
/// letters, digits, `.`, and `-` (covers plain hostnames and IDNA/punycode
/// labels like `xn--foo`). Rejects anything that could alter URL structure
/// (`/ ? # & @ % :`, whitespace, CR/LF, etc.) — the classic injection/SSRF
/// vector for a raw string splice.
fn validate_domain(domain: &str) -> Result<(), DnsError> {
    if domain.is_empty() {
        return Err(DnsError::Parse("domain must not be empty".into()));
    }
    if !domain
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
    {
        return Err(DnsError::Parse(format!(
            "domain '{domain}' contains characters outside the allowed hostname set [A-Za-z0-9.-]"
        )));
    }
    Ok(())
}

/// Build the request URL.
///
/// An explicit `urlTemplate` always wins — it lets an operator supply an
/// account-scoped / signed / correct-scheme endpoint while still reusing a
/// preset's response format (see [`PreparedResponse::from_config`], which keeps the
/// preset's format). A preset's built-in URL is only the fallback when no
/// `urlTemplate` is given.
pub fn render_url(cfg: &HttpDnsResolverConfig, domain: &str) -> Result<String, DnsError> {
    validate_domain(domain)?;
    if let Some(tmpl) = cfg.url_template.as_deref().filter(|s| !s.is_empty()) {
        if !tmpl.contains("{domain}") {
            return Err(DnsError::Parse("urlTemplate missing {domain}".into()));
        }
        return Ok(tmpl.replace("{domain}", domain));
    }
    if let Some(preset) = &cfg.preset {
        return Ok(preset.url_template().replace("{domain}", domain));
    }
    Err(DnsError::Parse("no urlTemplate and no preset configured".into()))
}

/// Query capability and deadline are compiled once with the provider generation.
pub(super) struct PreparedQuery {
    response: PreparedResponse,
    mode: QueryMode,
    timeout: std::time::Duration,
}

#[derive(Clone, Copy)]
enum QueryMode {
    Typed,
    Combined,
    Ipv4Preset,
}

impl PreparedQuery {
    pub(super) fn from_config(cfg: &HttpDnsResolverConfig) -> Self {
        let mode = match cfg.url_template.as_deref().filter(|url| !url.is_empty()) {
            Some(url) if url.contains("{recordType}") => QueryMode::Typed,
            Some(_) => QueryMode::Combined,
            None if cfg.preset.is_some() => QueryMode::Ipv4Preset,
            None => QueryMode::Combined,
        };
        Self {
            response: PreparedResponse::from_config(cfg),
            mode,
            timeout: primary_timeout(cfg),
        }
    }
}

/// Immutable interpretation metadata owned by one prepared query generation.
pub(super) enum PreparedResponse {
    Json {
        path: String,
        pointer: String,
        delimiter: Option<String>,
        /// JSON pointer of the provider's answer TTL (seconds), from `ttlPath`.
        ttl_pointer: Option<String>,
    },
    Delimited {
        delimiter: Option<String>,
    },
}

impl PreparedResponse {
    /// Preset response policy wins independently of an explicit URL template.
    pub(super) fn from_config(config: &HttpDnsResolverConfig) -> Self {
        match &config.preset {
            Some(preset) => Self::from_format(&preset.response_format()),
            None => Self::from_format(&config.response),
        }
    }

    fn from_format(format: &HttpDnsResponseFormat) -> Self {
        let delimiter = format.delimiter.clone();
        match format.kind {
            HttpDnsBodyKind::Json => {
                let path = format.ip_path.as_deref().unwrap_or("ips");
                Self::Json {
                    path: path.to_string(),
                    pointer: json_pointer_path(path),
                    delimiter,
                    ttl_pointer: format
                        .ttl_path
                        .as_deref()
                        .filter(|path| !path.is_empty())
                        .map(json_pointer_path),
                }
            }
            HttpDnsBodyKind::Delimited => Self::Delimited { delimiter },
        }
    }
}

fn json_pointer_path(path: &str) -> String {
    // "data.ips" -> "/data/ips"
    let mut p = String::new();
    for seg in path.split('.') {
        p.push('/');
        for ch in seg.chars() {
            match ch {
                '~' => p.push_str("~0"),
                '/' => p.push_str("~1"),
                _ => p.push(ch),
            }
        }
    }
    p
}

/// Reject an explicitly-empty delimiter (`Some("")`) before it is used to
/// `str::split` — an empty pattern splits into per-char garbage instead of
/// the intended tokens. `None` (default `,`) is unaffected.
fn require_non_empty_delimiter(delimiter: Option<&str>) -> Result<&str, DnsError> {
    match delimiter {
        Some("") => Err(DnsError::Parse("delimiter must not be empty".into())),
        Some(d) => Ok(d),
        None => Ok(","),
    }
}

/// Parse the response body into a de-duplicated IP list, capped at
/// [`MAX_HTTP_DNS_ADDRS`].
pub fn parse_body(fmt: &HttpDnsResponseFormat, body: &str) -> Result<Vec<IpAddr>, DnsError> {
    PreparedResponse::from_format(fmt)
        .parse(body)
        .map(|answer| answer.addrs)
}

/// Read a provider TTL in seconds: a non-negative integer number or an integer
/// string. Anything else is ignored — a TTL is advisory and never fails a lookup.
fn ttl_at(value: &serde_json::Value, pointer: Option<&str>) -> Option<Duration> {
    let node = value.pointer(pointer?)?;
    let seconds = match node {
        serde_json::Value::Number(number) => number.as_u64()?,
        serde_json::Value::String(text) => text.trim().parse::<u64>().ok()?,
        _ => return None,
    };
    Some(Duration::from_secs(seconds))
}

impl PreparedResponse {
    fn parse(&self, body: &str) -> Result<DnsAnswer, DnsError> {
        match self {
            Self::Json {
                path,
                pointer,
                delimiter,
                ttl_pointer,
            } => {
                let v: serde_json::Value = serde_json::from_str(body).map_err(|e| DnsError::Parse(e.to_string()))?;
                let node = v
                    .pointer(pointer)
                    .ok_or_else(|| DnsError::Parse(format!("ipPath '{path}' not found in response")))?;
                let addrs = match node {
                    serde_json::Value::Array(items) => parse_ip_candidates(items.iter().filter_map(|i| i.as_str())),
                    serde_json::Value::String(s) => {
                        let delim = require_non_empty_delimiter(delimiter.as_deref())?;
                        parse_ip_candidates(s.split(delim).map(str::trim))
                    }
                    _ => Err(DnsError::Parse("ipPath did not point to array/string".into())),
                }?;
                Ok(DnsAnswer {
                    addrs,
                    ttl: ttl_at(&v, ttl_pointer.as_deref()),
                })
            }
            Self::Delimited { delimiter } => {
                let delim = require_non_empty_delimiter(delimiter.as_deref())?;
                parse_ip_candidates(body.split(delim).map(str::trim)).map(DnsAnswer::without_ttl)
            }
        }
    }
}

fn parse_ip_candidates<'a>(raw: impl Iterator<Item = &'a str>) -> Result<Vec<IpAddr>, DnsError> {
    let mut out = Vec::new();
    let mut seen: HashSet<IpAddr> = HashSet::new();
    for s in raw {
        if out.len() >= MAX_HTTP_DNS_ADDRS {
            break;
        }
        let parsed = s.parse::<IpAddr>().or_else(|error| {
            let unbracketed = strip_brackets(s);
            if unbracketed.len() == s.len() {
                Err(error)
            } else {
                unbracketed.parse::<IpAddr>()
            }
        });
        if let Ok(ip) = parsed {
            if seen.insert(ip) {
                out.push(ip);
            }
        }
    }
    if out.is_empty() {
        Err(DnsError::NoAddresses)
    } else {
        Ok(out)
    }
}

/// One deadline covers the complete primary query, including both typed requests.
pub(super) fn primary_timeout(cfg: &HttpDnsResolverConfig) -> std::time::Duration {
    use edgion_resources::resources::link_sys::httpdns::HttpDnsConnection;
    std::time::Duration::from_millis(
        cfg.connection
            .as_ref()
            .and_then(|c| c.timeout_ms)
            .unwrap_or(HttpDnsConnection::DEFAULT_TIMEOUT_MS),
    )
}

impl PreparedResponse {
    /// Complete family-aware answers reject malformed values and overflow before filtering.
    fn parse_complete(&self, body: &str) -> Result<DnsAnswer, DnsError> {
        let mut addresses = Vec::new();
        let mut ttl = None;
        let mut insert = |raw: &str| -> Result<(), DnsError> {
            let ip = strip_brackets(raw.trim())
                .parse::<IpAddr>()
                .map_err(|_| DnsError::Parse("invalid IP address in DNS response".into()))?
                .to_canonical();
            if !addresses.contains(&ip) {
                addresses.push(ip);
            }
            if addresses.len() > MAX_HTTP_DNS_ADDRS {
                return Err(DnsError::Parse("DNS address limit exceeded".into()));
            }
            Ok(())
        };
        let mut parse_string = |value: &str, delimiter: Option<&str>| -> Result<(), DnsError> {
            let delimiter = require_non_empty_delimiter(delimiter)?;
            if !value.trim().is_empty() {
                for part in value.split(delimiter) {
                    insert(part)?;
                }
            }
            Ok(())
        };
        match self {
            Self::Delimited { delimiter } => parse_string(body, delimiter.as_deref())?,
            Self::Json {
                pointer,
                delimiter,
                ttl_pointer,
                ..
            } => {
                let value: serde_json::Value =
                    serde_json::from_str(body).map_err(|_| DnsError::Parse("invalid JSON DNS response".into()))?;
                ttl = ttl_at(&value, ttl_pointer.as_deref());
                match value.pointer(pointer) {
                    Some(serde_json::Value::String(value)) => parse_string(value, delimiter.as_deref())?,
                    Some(serde_json::Value::Array(values)) => {
                        for value in values {
                            insert(
                                value
                                    .as_str()
                                    .ok_or_else(|| DnsError::Parse("DNS address must be a string".into()))?,
                            )?;
                        }
                    }
                    _ => return Err(DnsError::Parse("missing or invalid DNS address container".into())),
                }
            }
        }
        Ok(DnsAnswer { addrs: addresses, ttl })
    }
}

async fn apply_fallback(
    fallback: &HttpDnsFallback,
    domain: &str,
    policy: DnsQueryPolicy,
) -> Result<DnsAnswer, DnsError> {
    match fallback {
        HttpDnsFallback::System => {
            let a = system::resolve_system(domain, policy)
                .await
                .map_err(|e| DnsError::Io(e.to_string()))?;
            require_addresses(a).map(DnsAnswer::without_ttl)
        }
        HttpDnsFallback::Dns { servers } => {
            // An empty server list is a misconfiguration, not "no DNS fallback
            // configured" — `resolve_standard` treats an empty list as "use the
            // system resolver", which would silently substitute a different
            // fallback than the operator declared. Fail explicitly instead.
            if servers.is_empty() {
                return Err(DnsError::NoAddresses);
            }
            standard::resolve_standard(servers, domain, policy).await
        }
        HttpDnsFallback::None => Err(DnsError::NoAddresses),
    }
}

/// Run only the configured fallback for `cfg`.
///
/// Used when the primary HTTP path cannot even be attempted — e.g. the outbound
/// client failed to build from bad resolved TLS material. Without this the caller
/// would surface the build error and silently skip the operator's configured
/// `fallback: system|dns`, which would be a fail-open surprise.
pub async fn resolve_via_fallback(
    cfg: &HttpDnsResolverConfig,
    domain: &str,
    policy: DnsQueryPolicy,
) -> Result<DnsAnswer, DnsError> {
    apply_fallback(&cfg.fallback, domain, policy).await
}

/// Core logic with an injectable fetcher (used by tests and the real entry point).
pub async fn resolve_http_dns_with<F: HttpDnsFetcher + ?Sized>(
    fetcher: &F,
    cfg: &HttpDnsResolverConfig,
    domain: &str,
    policy: DnsQueryPolicy,
) -> Result<DnsAnswer, DnsError> {
    let query = PreparedQuery::from_config(cfg);
    resolve_http_dns_prepared(fetcher, cfg, &query, domain, policy).await
}

pub(super) async fn resolve_http_dns_prepared<F: HttpDnsFetcher + ?Sized>(
    fetcher: &F,
    cfg: &HttpDnsResolverConfig,
    query: &PreparedQuery,
    domain: &str,
    policy: DnsQueryPolicy,
) -> Result<DnsAnswer, DnsError> {
    let primary = async {
        let url = render_url(cfg, domain)?;
        if policy == DnsQueryPolicy::Native {
            let body = fetcher.get(&url.replace("{recordType}", "A")).await?;
            return query.response.parse(&body);
        }
        if matches!(query.mode, QueryMode::Ipv4Preset) && policy != DnsQueryPolicy::Family(DnsLookupFamily::Ipv4Only) {
            return Err(DnsError::Parse("preset requires a custom URL for IPv6 queries".into()));
        }
        let mut addresses = Vec::new();
        let mut ttl = None;
        if matches!(query.mode, QueryMode::Typed) {
            let families: &[(DnsLookupFamily, &str)] = match policy {
                DnsQueryPolicy::Family(DnsLookupFamily::Ipv4Only) => &[(DnsLookupFamily::Ipv4Only, "A")],
                DnsQueryPolicy::Family(DnsLookupFamily::Ipv6Only) => &[(DnsLookupFamily::Ipv6Only, "AAAA")],
                _ => &[(DnsLookupFamily::Ipv4Only, "A"), (DnsLookupFamily::Ipv6Only, "AAAA")],
            };
            for &(family, record_type) in families {
                let body = fetcher.get(&url.replace("{recordType}", record_type)).await?;
                let parsed = query.response.parse_complete(&body)?;
                if parsed
                    .addrs
                    .iter()
                    .any(|ip| !DnsQueryPolicy::Family(family).accepts(*ip))
                {
                    return Err(DnsError::Parse(
                        "typed DNS response contains the wrong address family".into(),
                    ));
                }
                // An empty family answer does not bound the lifetime of the other family.
                if !parsed.addrs.is_empty() {
                    ttl = min_ttl(ttl, parsed.ttl);
                }
                for ip in parsed.addrs {
                    if !addresses.contains(&ip) {
                        addresses.push(ip);
                    }
                }
                if addresses.len() > MAX_HTTP_DNS_ADDRS {
                    return Err(DnsError::Parse("DNS address limit exceeded".into()));
                }
            }
        } else {
            let body = fetcher.get(&url).await?;
            let parsed = query.response.parse_complete(&body)?;
            addresses = parsed.addrs;
            ttl = parsed.ttl;
        }
        require_addresses(policy.select(addresses)).map(|addrs| DnsAnswer { addrs, ttl })
    };
    let attempt = if policy == DnsQueryPolicy::Native {
        primary.await
    } else {
        tokio::time::timeout(query.timeout, primary)
            .await
            .unwrap_or_else(|_| Err(DnsError::Http("primary DNS deadline exceeded".into())))
    };

    match attempt {
        Ok(ips) => Ok(ips),
        Err(primary) => match apply_fallback(&cfg.fallback, domain, policy).await {
            Ok(ips) => Ok(ips),
            Err(_) => Err(primary),
        },
    }
}

#[cfg(test)]
#[path = "../../tests/unit/dns/httpdns_test.rs"]
mod tests;
