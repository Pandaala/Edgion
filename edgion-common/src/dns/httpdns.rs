//! HTTP DNS backend: render a request URL, fetch it, parse the response into
//! IPs, and apply a configured fallback on failure.

use std::collections::HashSet;
use std::net::IpAddr;

use edgion_resources::resources::link_sys::httpdns::{
    HttpDnsBodyKind, HttpDnsFallback, HttpDnsResolverConfig, HttpDnsResponseFormat,
};

use super::{standard, strip_brackets, system, DnsError};

/// Hard cap on the number of IPs accepted from a single HTTP DNS response.
/// Once reached, further candidates are silently dropped (not an error) —
/// this bounds memory/response processing against a malicious or buggy
/// upstream that returns an unbounded IP list.
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
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
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

/// Immutable interpretation metadata owned by one prepared query generation.
pub(super) enum PreparedResponse {
    Json {
        path: String,
        pointer: String,
        delimiter: Option<String>,
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
    PreparedResponse::from_format(fmt).parse(body)
}

impl PreparedResponse {
    fn parse(&self, body: &str) -> Result<Vec<IpAddr>, DnsError> {
        match self {
            Self::Json {
                path,
                pointer,
                delimiter,
            } => {
                let v: serde_json::Value = serde_json::from_str(body).map_err(|e| DnsError::Parse(e.to_string()))?;
                let node = v
                    .pointer(pointer)
                    .ok_or_else(|| DnsError::Parse(format!("ipPath '{path}' not found in response")))?;
                match node {
                    serde_json::Value::Array(items) => parse_ip_candidates(items.iter().filter_map(|i| i.as_str())),
                    serde_json::Value::String(s) => {
                        let delim = require_non_empty_delimiter(delimiter.as_deref())?;
                        parse_ip_candidates(s.split(delim).map(str::trim))
                    }
                    _ => Err(DnsError::Parse("ipPath did not point to array/string".into())),
                }
            }
            Self::Delimited { delimiter } => {
                let delim = require_non_empty_delimiter(delimiter.as_deref())?;
                parse_ip_candidates(body.split(delim).map(str::trim))
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

async fn apply_fallback(fallback: &HttpDnsFallback, domain: &str) -> Result<Vec<IpAddr>, DnsError> {
    match fallback {
        HttpDnsFallback::System => {
            let a = system::resolve_system(domain)
                .await
                .map_err(|e| DnsError::Io(e.to_string()))?;
            if a.is_empty() {
                Err(DnsError::NoAddresses)
            } else {
                Ok(a)
            }
        }
        HttpDnsFallback::Dns { servers } => {
            // An empty server list is a misconfiguration, not "no DNS fallback
            // configured" — `resolve_standard` treats an empty list as "use the
            // system resolver", which would silently substitute a different
            // fallback than the operator declared. Fail explicitly instead.
            if servers.is_empty() {
                return Err(DnsError::NoAddresses);
            }
            standard::resolve_standard(servers, domain).await
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
pub async fn resolve_via_fallback(cfg: &HttpDnsResolverConfig, domain: &str) -> Result<Vec<IpAddr>, DnsError> {
    apply_fallback(&cfg.fallback, domain).await
}

/// Core logic with an injectable fetcher (used by tests and the real entry point).
pub async fn resolve_http_dns_with<F: HttpDnsFetcher + ?Sized>(
    fetcher: &F,
    cfg: &HttpDnsResolverConfig,
    domain: &str,
) -> Result<Vec<IpAddr>, DnsError> {
    let response = PreparedResponse::from_config(cfg);
    resolve_http_dns_prepared(fetcher, cfg, &response, domain).await
}

pub(super) async fn resolve_http_dns_prepared<F: HttpDnsFetcher + ?Sized>(
    fetcher: &F,
    cfg: &HttpDnsResolverConfig,
    response: &PreparedResponse,
    domain: &str,
) -> Result<Vec<IpAddr>, DnsError> {
    let attempt = async {
        let url = render_url(cfg, domain)?;
        let body = fetcher.get(&url).await?;
        response.parse(&body)
    }
    .await;

    match attempt {
        Ok(ips) => Ok(ips),
        Err(primary) => match apply_fallback(&cfg.fallback, domain).await {
            Ok(ips) => Ok(ips),
            Err(_) => Err(primary),
        },
    }
}

#[cfg(test)]
#[path = "../../tests/unit/dns/httpdns_test.rs"]
mod tests;
