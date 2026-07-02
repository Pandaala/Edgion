//! HTTP DNS backend: render a request URL, fetch it, parse the response into
//! IPs, and apply a configured fallback on failure.

use std::collections::HashSet;
use std::net::IpAddr;

use edgion_resources::resources::link_sys::httpdns::{
    HttpDnsBodyKind, HttpDnsFallback, HttpDnsPreset, HttpDnsResolverConfig, HttpDnsResponseFormat,
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

/// Build the request URL: preset templates take priority, else substitute `{domain}`.
pub fn render_url(cfg: &HttpDnsResolverConfig, domain: &str) -> Result<String, DnsError> {
    validate_domain(domain)?;
    if let Some(preset) = &cfg.preset {
        let tmpl = match preset {
            // Placeholder preset templates — replace host/account params at wire-up
            // time with the real vendor endpoints; the {domain} contract is what matters.
            HttpDnsPreset::Aliyun => "https://203.0.113.1/resolve?host={domain}",
            HttpDnsPreset::Tencent => "https://203.0.113.2/d?dn={domain}",
        };
        return Ok(tmpl.replace("{domain}", domain));
    }
    let tmpl = cfg.url_template.as_deref().unwrap_or("");
    if !tmpl.contains("{domain}") {
        return Err(DnsError::Parse("urlTemplate missing {domain}".into()));
    }
    Ok(tmpl.replace("{domain}", domain))
}

fn json_pointer_path(path: &str) -> String {
    // "data.ips" -> "/data/ips"
    let mut p = String::new();
    for seg in path.split('.') {
        p.push('/');
        p.push_str(seg);
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
    let raw: Vec<String> = match fmt.kind {
        HttpDnsBodyKind::Json => {
            let v: serde_json::Value = serde_json::from_str(body).map_err(|e| DnsError::Parse(e.to_string()))?;
            let path = fmt.ip_path.as_deref().unwrap_or("ips");
            let node = v
                .pointer(&json_pointer_path(path))
                .ok_or_else(|| DnsError::Parse(format!("ipPath '{path}' not found in response")))?;
            match node {
                serde_json::Value::Array(items) => {
                    items.iter().filter_map(|i| i.as_str().map(|s| s.to_string())).collect()
                }
                serde_json::Value::String(s) => {
                    let delim = require_non_empty_delimiter(fmt.delimiter.as_deref())?;
                    s.split(delim).map(|x| x.trim().to_string()).collect()
                }
                _ => return Err(DnsError::Parse("ipPath did not point to array/string".into())),
            }
        }
        HttpDnsBodyKind::Delimited => {
            let delim = require_non_empty_delimiter(fmt.delimiter.as_deref())?;
            body.split(delim).map(|x| x.trim().to_string()).collect()
        }
    };

    let mut out = Vec::new();
    let mut seen: HashSet<IpAddr> = HashSet::new();
    for s in raw {
        if out.len() >= MAX_HTTP_DNS_ADDRS {
            break;
        }
        let parsed = s.parse::<IpAddr>().or_else(|_| strip_brackets(&s).parse::<IpAddr>());
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

/// Core logic with an injectable fetcher (used by tests and the real entry point).
pub async fn resolve_http_dns_with<F: HttpDnsFetcher + ?Sized>(
    fetcher: &F,
    cfg: &HttpDnsResolverConfig,
    domain: &str,
) -> Result<Vec<IpAddr>, DnsError> {
    let attempt = async {
        let url = render_url(cfg, domain)?;
        let body = fetcher.get(&url).await?;
        parse_body(&cfg.response, &body)
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
mod tests {
    use super::*;
    use edgion_resources::resources::link_sys::httpdns::{
        HttpDnsBodyKind, HttpDnsFallback, HttpDnsPreset, HttpDnsResolverConfig, HttpDnsResponseFormat,
    };

    fn generic(url: &str, fmt: HttpDnsResponseFormat) -> HttpDnsResolverConfig {
        HttpDnsResolverConfig {
            preset: None,
            url_template: Some(url.to_string()),
            response: fmt,
            fallback: HttpDnsFallback::None,
            connection: None,
        }
    }

    #[test]
    fn render_url_substitutes_domain() {
        let cfg = generic("https://h/d?host={domain}", HttpDnsResponseFormat::default());
        assert_eq!(
            render_url(&cfg, "api.example.com").unwrap(),
            "https://h/d?host=api.example.com"
        );
    }

    #[test]
    fn render_url_preset_ignores_missing_template() {
        let mut cfg = generic("", HttpDnsResponseFormat::default());
        cfg.preset = Some(HttpDnsPreset::Aliyun);
        cfg.url_template = None;
        assert!(render_url(&cfg, "api.example.com").unwrap().contains("api.example.com"));
    }

    #[test]
    fn parse_json_ip_array() {
        let fmt = HttpDnsResponseFormat {
            kind: HttpDnsBodyKind::Json,
            ip_path: Some("ips".to_string()),
            delimiter: None,
            ttl_path: None,
        };
        let ips = parse_body(&fmt, r#"{"ips":["93.184.216.34","93.184.216.35"]}"#).unwrap();
        assert_eq!(
            ips,
            vec![
                "93.184.216.34".parse::<IpAddr>().unwrap(),
                "93.184.216.35".parse::<IpAddr>().unwrap()
            ]
        );
    }

    #[test]
    fn parse_json_nested_path() {
        let fmt = HttpDnsResponseFormat {
            kind: HttpDnsBodyKind::Json,
            ip_path: Some("data.ips".to_string()),
            delimiter: None,
            ttl_path: None,
        };
        let ips = parse_body(&fmt, r#"{"data":{"ips":["1.2.3.4"]}}"#).unwrap();
        assert_eq!(ips, vec!["1.2.3.4".parse::<IpAddr>().unwrap()]);
    }

    #[test]
    fn parse_delimited() {
        let fmt = HttpDnsResponseFormat {
            kind: HttpDnsBodyKind::Delimited,
            ip_path: None,
            delimiter: Some(",".to_string()),
            ttl_path: None,
        };
        let ips = parse_body(&fmt, "1.2.3.4,5.6.7.8").unwrap();
        assert_eq!(
            ips,
            vec![
                "1.2.3.4".parse::<IpAddr>().unwrap(),
                "5.6.7.8".parse::<IpAddr>().unwrap()
            ]
        );
    }

    #[test]
    fn parse_rejects_no_ips() {
        let fmt = HttpDnsResponseFormat {
            kind: HttpDnsBodyKind::Delimited,
            ip_path: None,
            delimiter: Some(",".to_string()),
            ttl_path: None,
        };
        assert!(matches!(
            parse_body(&fmt, "not-an-ip"),
            Err(DnsError::Parse(_)) | Err(DnsError::NoAddresses)
        ));
    }

    // ---- DR-2: render_url domain validation ----

    #[test]
    fn render_url_rejects_ampersand_injection() {
        let cfg = generic("https://h/d?host={domain}", HttpDnsResponseFormat::default());
        assert!(matches!(render_url(&cfg, "a&b=1"), Err(DnsError::Parse(_))));
    }

    #[test]
    fn render_url_rejects_path_traversal_slash() {
        let cfg = generic("https://h/d?host={domain}", HttpDnsResponseFormat::default());
        assert!(matches!(render_url(&cfg, "x/../y"), Err(DnsError::Parse(_))));
    }

    #[test]
    fn render_url_rejects_whitespace() {
        let cfg = generic("https://h/d?host={domain}", HttpDnsResponseFormat::default());
        assert!(matches!(render_url(&cfg, "h ost"), Err(DnsError::Parse(_))));
    }

    #[test]
    fn render_url_rejects_at_sign() {
        let cfg = generic("https://h/d?host={domain}", HttpDnsResponseFormat::default());
        assert!(matches!(render_url(&cfg, "a@b"), Err(DnsError::Parse(_))));
    }

    #[test]
    fn render_url_rejects_empty_domain() {
        let cfg = generic("https://h/d?host={domain}", HttpDnsResponseFormat::default());
        assert!(matches!(render_url(&cfg, ""), Err(DnsError::Parse(_))));
    }

    #[test]
    fn render_url_accepts_normal_domain() {
        let cfg = generic("https://h/d?host={domain}", HttpDnsResponseFormat::default());
        assert_eq!(
            render_url(&cfg, "api.example.com").unwrap(),
            "https://h/d?host=api.example.com"
        );
    }

    #[test]
    fn render_url_accepts_idna_style_domain() {
        let cfg = generic("https://h/d?host={domain}", HttpDnsResponseFormat::default());
        assert_eq!(
            render_url(&cfg, "xn--foo.example.com").unwrap(),
            "https://h/d?host=xn--foo.example.com"
        );
    }

    // ---- DR-3: parse_body cap + dedup + empty-delimiter rejection ----

    #[test]
    fn parse_delimited_caps_at_max_addrs() {
        let ips: Vec<String> = (0..300u32)
            .map(|i| format!("10.{}.{}.{}", (i >> 16) & 0xff, (i >> 8) & 0xff, i & 0xff))
            .collect();
        let body = ips.join(",");
        let fmt = HttpDnsResponseFormat {
            kind: HttpDnsBodyKind::Delimited,
            ip_path: None,
            delimiter: Some(",".to_string()),
            ttl_path: None,
        };
        let out = parse_body(&fmt, &body).unwrap();
        assert_eq!(out.len(), MAX_HTTP_DNS_ADDRS);
    }

    #[test]
    fn parse_delimited_rejects_empty_delimiter() {
        let fmt = HttpDnsResponseFormat {
            kind: HttpDnsBodyKind::Delimited,
            ip_path: None,
            delimiter: Some(String::new()),
            ttl_path: None,
        };
        assert!(matches!(parse_body(&fmt, "1.2.3.4"), Err(DnsError::Parse(_))));
    }

    #[test]
    fn parse_json_string_value_rejects_empty_delimiter() {
        let fmt = HttpDnsResponseFormat {
            kind: HttpDnsBodyKind::Json,
            ip_path: Some("ips".to_string()),
            delimiter: Some(String::new()),
            ttl_path: None,
        };
        assert!(matches!(
            parse_body(&fmt, r#"{"ips":"1.2.3.4,5.6.7.8"}"#),
            Err(DnsError::Parse(_))
        ));
    }

    #[test]
    fn parse_delimited_dedups_preserving_order() {
        let fmt = HttpDnsResponseFormat {
            kind: HttpDnsBodyKind::Delimited,
            ip_path: None,
            delimiter: Some(",".to_string()),
            ttl_path: None,
        };
        let ips = parse_body(&fmt, "1.2.3.4,5.6.7.8,1.2.3.4").unwrap();
        assert_eq!(
            ips,
            vec![
                "1.2.3.4".parse::<IpAddr>().unwrap(),
                "5.6.7.8".parse::<IpAddr>().unwrap()
            ]
        );
    }

    #[test]
    fn parse_delimited_accepts_bracketed_ipv6() {
        let fmt = HttpDnsResponseFormat {
            kind: HttpDnsBodyKind::Delimited,
            ip_path: None,
            delimiter: Some(",".to_string()),
            ttl_path: None,
        };
        let ips = parse_body(&fmt, "[::1],[2001:db8::1]").unwrap();
        assert_eq!(
            ips,
            vec![
                "::1".parse::<IpAddr>().unwrap(),
                "2001:db8::1".parse::<IpAddr>().unwrap()
            ]
        );
    }

    // ---- apply_fallback: empty Dns { servers } must not silently use the system resolver ----

    #[tokio::test]
    async fn apply_fallback_empty_dns_servers_errors_directly() {
        let err = apply_fallback(&HttpDnsFallback::Dns { servers: vec![] }, "example.com")
            .await
            .unwrap_err();
        assert_eq!(err, DnsError::NoAddresses);
    }

    #[tokio::test]
    async fn fetch_failure_empty_dns_fallback_servers_errors() {
        let mut cfg = generic("https://h/d?host={domain}", HttpDnsResponseFormat::default());
        cfg.fallback = HttpDnsFallback::Dns { servers: vec![] };
        let err = resolve_http_dns_with(&FailingFetcher, &cfg, "example.com")
            .await
            .unwrap_err();
        // The primary failure (fetch error) is what surfaces, since the fallback
        // itself errors too — but the fallback path must not have silently
        // succeeded via the system resolver.
        assert!(matches!(err, DnsError::Http(_)));
    }

    // Fallback: a fetcher that always fails must trigger cfg.fallback.
    struct FailingFetcher;
    #[async_trait::async_trait]
    impl HttpDnsFetcher for FailingFetcher {
        async fn get(&self, _url: &str) -> Result<String, DnsError> {
            Err(DnsError::Http("boom".into()))
        }
    }

    #[tokio::test]
    async fn fetch_failure_uses_system_fallback() {
        let mut cfg = generic("https://h/d?host={domain}", HttpDnsResponseFormat::default());
        cfg.fallback = HttpDnsFallback::System;
        let ips = resolve_http_dns_with(&FailingFetcher, &cfg, "localhost").await.unwrap();
        assert!(ips.iter().any(|ip| ip.is_loopback()));
    }

    #[tokio::test]
    async fn fetch_failure_none_fallback_errors() {
        let cfg = generic("https://h/d?host={domain}", HttpDnsResponseFormat::default());
        assert!(resolve_http_dns_with(&FailingFetcher, &cfg, "localhost").await.is_err());
    }
}
