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
fn render_url_falls_back_to_preset_when_no_template() {
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

// ---- preset drives BOTH url template AND response format ----

/// A fetcher that returns a fixed body regardless of URL.
struct FixedBodyFetcher(&'static str);
#[async_trait::async_trait]
impl HttpDnsFetcher for FixedBodyFetcher {
    async fn get(&self, _url: &str) -> Result<String, DnsError> {
        Ok(self.0.to_string())
    }
}

fn preset_cfg(preset: HttpDnsPreset) -> HttpDnsResolverConfig {
    HttpDnsResolverConfig {
        preset: Some(preset),
        // A bogus/absent user response must NOT be consulted when a preset is set.
        url_template: None,
        response: HttpDnsResponseFormat {
            kind: HttpDnsBodyKind::Delimited,
            ip_path: None,
            delimiter: Some("|".to_string()),
            ttl_path: None,
        },
        fallback: HttpDnsFallback::None,
        connection: None,
    }
}

#[test]
fn render_url_uses_aliyun_preset_host() {
    let cfg = preset_cfg(HttpDnsPreset::Aliyun);
    let url = render_url(&cfg, "api.example.com").unwrap();
    assert!(url.contains("203.107.1.1"), "aliyun preset host, got {url}");
    assert!(url.ends_with("host=api.example.com"));
}

#[tokio::test]
async fn preset_plus_url_template_uses_custom_url_and_preset_format() {
    // preset = Aliyun (JSON `ips` format), but an explicit account-scoped
    // urlTemplate is supplied — the URL must come from the template while the
    // response is still parsed with the Aliyun JSON format.
    let mut cfg = preset_cfg(HttpDnsPreset::Aliyun);
    cfg.url_template = Some("http://203.107.1.1/my-account/d?host={domain}".to_string());

    let url = render_url(&cfg, "api.example.com").unwrap();
    assert_eq!(url, "http://203.107.1.1/my-account/d?host=api.example.com");

    // Body is Aliyun-shaped JSON; the preset format (not the delimited user
    // `response`) must parse it.
    let fetcher = FixedBodyFetcher(r#"{"host":"api.example.com","ips":["93.184.216.34"],"ttl":30}"#);
    let ips = resolve_http_dns_with(&fetcher, &cfg, "api.example.com").await.unwrap();
    assert_eq!(ips, vec!["93.184.216.34".parse::<IpAddr>().unwrap()]);
}

#[tokio::test]
async fn aliyun_preset_parses_json_ips_ignoring_user_response() {
    let cfg = preset_cfg(HttpDnsPreset::Aliyun);
    // Aliyun shape: JSON with an `ips` array. The user `response` (delimited `|`)
    // must be ignored in favor of the preset's JSON format.
    let fetcher = FixedBodyFetcher(r#"{"host":"api.example.com","ips":["93.184.216.34"],"ttl":30}"#);
    let ips = resolve_http_dns_with(&fetcher, &cfg, "api.example.com").await.unwrap();
    assert_eq!(ips, vec!["93.184.216.34".parse::<IpAddr>().unwrap()]);
}

#[tokio::test]
async fn tencent_preset_parses_semicolon_delimited_ignoring_user_response() {
    let cfg = preset_cfg(HttpDnsPreset::Tencent);
    // Tencent DNSPod shape: semicolon-delimited plaintext.
    let fetcher = FixedBodyFetcher("1.2.3.4;5.6.7.8");
    let ips = resolve_http_dns_with(&fetcher, &cfg, "api.example.com").await.unwrap();
    assert_eq!(
        ips,
        vec![
            "1.2.3.4".parse::<IpAddr>().unwrap(),
            "5.6.7.8".parse::<IpAddr>().unwrap()
        ]
    );
}

#[test]
fn dotted_json_path_preserves_pointer_metacharacters() {
    let fmt = HttpDnsResponseFormat {
        ip_path: Some("data.a~/b.0".to_string()),
        ..Default::default()
    };
    let ips = parse_body(&fmt, r#"{"data":{"a~/b":[["192.0.2.1"]],"a~":{"b":[["192.0.2.2"]]}}}"#).unwrap();
    assert_eq!(ips, vec!["192.0.2.1".parse::<IpAddr>().unwrap()]);
}

#[test]
fn borrowed_candidates_preserve_format_specific_whitespace_and_order() {
    let fmt = HttpDnsResponseFormat::default();
    let ips = parse_body(
        &fmt,
        r#"{"ips":[" 192.0.2.1 ",null,"bad","192.0.2.2","192.0.2.2","[::1]"]}"#,
    )
    .unwrap();
    assert_eq!(
        ips,
        vec!["192.0.2.2".parse::<IpAddr>().unwrap(), "::1".parse().unwrap()]
    );
    let ips = parse_body(&fmt, r#"{"ips":" 192.0.2.1 ,bad,192.0.2.1, [::1] "}"#).unwrap();
    assert_eq!(
        ips,
        vec!["192.0.2.1".parse::<IpAddr>().unwrap(), "::1".parse().unwrap()]
    );
}
