use super::*;

struct TypedFetcher {
    v4: &'static str,
    v6: &'static str,
}

#[async_trait::async_trait]
impl HttpDnsFetcher for TypedFetcher {
    async fn get(&self, url: &str) -> Result<String, DnsError> {
        Ok(if url.ends_with("=AAAA") { self.v6 } else { self.v4 }.to_owned())
    }
}

#[tokio::test]
async fn complete_dual_answer_distinguishes_empty_from_failed_family() {
    let cfg = generic(
        "https://dns.test/?host={domain}&type={recordType}",
        HttpDnsResponseFormat::default(),
    );
    let policy = DnsQueryPolicy::Family(DnsLookupFamily::All);
    let fetcher = TypedFetcher {
        v4: r#"{"ips":["192.0.2.1"]}"#,
        v6: r#"{"ips":[]}"#,
    };
    assert_eq!(
        resolve_http_dns_with(&fetcher, &cfg, "backend.test", policy)
            .await
            .unwrap()
            .addrs,
        vec!["192.0.2.1".parse::<IpAddr>().unwrap()]
    );
    for v6 in [
        r#"{"ips":["bad"]}"#,
        "{}",
        r#"{"ips":["192.0.2.2"]}"#,
        r#"{"ips":[null]}"#,
    ] {
        assert!(
            resolve_http_dns_with(&TypedFetcher { v6, ..fetcher }, &cfg, "backend.test", policy)
                .await
                .is_err()
        );
    }
    let both = TypedFetcher {
        v4: r#"{"ips":[]}"#,
        v6: r#"{"ips":""}"#,
    };
    assert!(matches!(
        resolve_http_dns_with(&both, &cfg, "backend.test", policy).await,
        Err(DnsError::NoAddresses)
    ));
}

#[test]
fn complete_parser_rejects_overflow_and_canonicalizes_before_dedup() {
    let response = PreparedResponse::from_format(&HttpDnsResponseFormat::default());
    let ips: Vec<_> = (0..257).map(|n| format!("2001:db8::{n:x}")).collect();
    let body = serde_json::json!({"ips": ips}).to_string();
    assert!(response.parse_complete(&body).is_err());
    assert_eq!(
        response
            .parse_complete(r#"{"ips":["192.0.2.1","::ffff:192.0.2.1"]}"#)
            .unwrap()
            .addrs
            .len(),
        1
    );
}

#[tokio::test]
async fn preset_ipv6_does_not_query_unverified_vendor_url() {
    let cfg = preset_cfg(HttpDnsPreset::Aliyun);
    let fetcher = FixedBodyFetcher(r#"{"ips":["2001:db8::1"]}"#);
    assert!(resolve_http_dns_with(
        &fetcher,
        &cfg,
        "backend.test",
        DnsQueryPolicy::Family(DnsLookupFamily::Ipv6Only)
    )
    .await
    .is_err());
}

#[tokio::test]
async fn fallback_preserves_family_for_literal() {
    let mut cfg = generic("https://dns.test/{domain}", HttpDnsResponseFormat::default());
    cfg.fallback = HttpDnsFallback::System;
    assert!(resolve_http_dns_with(
        &FailingFetcher,
        &cfg,
        "127.0.0.1",
        DnsQueryPolicy::Family(DnsLookupFamily::Ipv6Only)
    )
    .await
    .is_err());
    assert_eq!(
        resolve_http_dns_with(
            &FailingFetcher,
            &cfg,
            "127.0.0.1",
            DnsQueryPolicy::Family(DnsLookupFamily::Ipv4Only)
        )
        .await
        .unwrap()
        .addrs
        .len(),
        1
    );
}
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
fn domain_validation_preserves_ascii_allowlist_and_errors() {
    let domains = (0u8..=127)
        .map(|value| format!("a{}.test", char::from(value)))
        .chain(["", "é.test", "ñ.test", "€.test", "π.test"].map(str::to_string));
    for domain in domains {
        let expected = if domain.is_empty() {
            Err(DnsError::Parse("domain must not be empty".into()))
        } else if domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        {
            Ok(())
        } else {
            Err(DnsError::Parse(format!(
                "domain '{domain}' contains characters outside the allowed hostname set [A-Za-z0-9.-]"
            )))
        };
        assert_eq!(validate_domain(&domain), expected, "input: {domain:?}");
    }
}

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

#[test]
fn candidate_parsing_preserves_legacy_results_and_consumption() {
    fn original<'a>(raw: impl Iterator<Item = &'a str>) -> Result<Vec<IpAddr>, DnsError> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for value in raw {
            if out.len() >= MAX_HTTP_DNS_ADDRS {
                break;
            }
            if let Ok(ip) = value.parse::<IpAddr>().or_else(|_| strip_brackets(value).parse()) {
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

    fn compare(values: &[String]) {
        let expected_reads = std::cell::Cell::new(0);
        let actual_reads = std::cell::Cell::new(0);
        let expected = original(values.iter().map(String::as_str).inspect(|_| {
            expected_reads.set(expected_reads.get() + 1);
        }));
        let actual = parse_ip_candidates(values.iter().map(String::as_str).inspect(|_| {
            actual_reads.set(actual_reads.get() + 1);
        }));
        assert_eq!(actual, expected);
        assert_eq!(actual_reads.get(), expected_reads.get());
    }

    let cases = [
        "",
        "invalid",
        "é",
        "\0",
        "127.0.0.1",
        "::1",
        "2001:db8::1",
        "::ffff:192.0.2.1",
        "[::1]",
        "[127.0.0.1]",
        "[]",
        "[[::1]]",
        "[::1",
        "::1]",
        "[invalid]",
        "[::1]:53",
        "127.0.0.1:53",
        "fe80::1%lo0",
        " ::1",
        "::1 ",
        "[ ::1]",
        "[::1 ]",
    ];
    compare(&[]);
    for value in cases {
        compare(&[value.to_string()]);
    }
    let mut mixed: Vec<String> = cases.into_iter().cycle().take(100).map(str::to_string).collect();
    compare(&mixed);
    mixed.extend((0..300).flat_map(|i| [format!("10.0.{}.{}", i / 256, i % 256), "invalid".into()]));
    compare(&mixed);
    let reads = std::cell::Cell::new(0);
    let unique: Vec<String> = (0..300).map(|i| format!("10.0.{}.{}", i / 256, i % 256)).collect();
    assert_eq!(
        parse_ip_candidates(
            unique
                .iter()
                .map(String::as_str)
                .inspect(|_| reads.set(reads.get() + 1))
        )
        .unwrap()
        .len(),
        MAX_HTTP_DNS_ADDRS
    );
    assert_eq!(reads.get(), MAX_HTTP_DNS_ADDRS + 1);
}

// ---- apply_fallback: empty Dns { servers } must not silently use the system resolver ----

#[tokio::test]
async fn apply_fallback_empty_dns_servers_errors_directly() {
    let err = apply_fallback(
        &HttpDnsFallback::Dns { servers: vec![] },
        "example.com",
        DnsQueryPolicy::Native,
    )
    .await
    .unwrap_err();
    assert_eq!(err, DnsError::NoAddresses);
}

#[tokio::test]
async fn fetch_failure_empty_dns_fallback_servers_errors() {
    let mut cfg = generic("https://h/d?host={domain}", HttpDnsResponseFormat::default());
    cfg.fallback = HttpDnsFallback::Dns { servers: vec![] };
    let err = resolve_http_dns_with(&FailingFetcher, &cfg, "example.com", crate::dns::DnsQueryPolicy::Native)
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
    let ips = resolve_http_dns_with(&FailingFetcher, &cfg, "localhost", crate::dns::DnsQueryPolicy::Native)
        .await
        .unwrap();
    assert!(ips.addrs.iter().any(|ip| ip.is_loopback()));
}

#[tokio::test]
async fn fetch_failure_none_fallback_errors() {
    let cfg = generic("https://h/d?host={domain}", HttpDnsResponseFormat::default());
    assert!(
        resolve_http_dns_with(&FailingFetcher, &cfg, "localhost", crate::dns::DnsQueryPolicy::Native)
            .await
            .is_err()
    );
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
    let ips = resolve_http_dns_with(&fetcher, &cfg, "api.example.com", crate::dns::DnsQueryPolicy::Native)
        .await
        .unwrap();
    assert_eq!(ips.addrs, vec!["93.184.216.34".parse::<IpAddr>().unwrap()]);
}

#[tokio::test]
async fn aliyun_preset_parses_json_ips_ignoring_user_response() {
    let cfg = preset_cfg(HttpDnsPreset::Aliyun);
    // Aliyun shape: JSON with an `ips` array. The user `response` (delimited `|`)
    // must be ignored in favor of the preset's JSON format.
    let fetcher = FixedBodyFetcher(r#"{"host":"api.example.com","ips":["93.184.216.34"],"ttl":30}"#);
    let ips = resolve_http_dns_with(&fetcher, &cfg, "api.example.com", crate::dns::DnsQueryPolicy::Native)
        .await
        .unwrap();
    assert_eq!(ips.addrs, vec!["93.184.216.34".parse::<IpAddr>().unwrap()]);
    assert_eq!(
        ips.ttl,
        Some(std::time::Duration::from_secs(30)),
        "the preset ttlPath is honored"
    );
}

#[tokio::test]
async fn tencent_preset_parses_semicolon_delimited_ignoring_user_response() {
    let cfg = preset_cfg(HttpDnsPreset::Tencent);
    // Tencent DNSPod shape: semicolon-delimited plaintext.
    let fetcher = FixedBodyFetcher("1.2.3.4;5.6.7.8");
    let ips = resolve_http_dns_with(&fetcher, &cfg, "api.example.com", crate::dns::DnsQueryPolicy::Native)
        .await
        .unwrap();
    assert_eq!(
        ips.addrs,
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

#[test]
fn prepared_response_keeps_branch_local_delimiter_checks_and_error_order() {
    let mut format = HttpDnsResponseFormat {
        kind: HttpDnsBodyKind::Json,
        ip_path: Some("data.ips".into()),
        delimiter: Some(String::new()),
        ttl_path: Some("unused.ttl".into()),
    };
    let response = PreparedResponse::from_format(&format);
    format.ip_path = Some("changed".into());
    for _ in 0..3 {
        assert_eq!(
            response.parse(r#"{"data":{"ips":["192.0.2.1"]}}"#).unwrap().addrs,
            vec!["192.0.2.1".parse::<IpAddr>().unwrap()]
        );
    }
    for (body, expected) in [
        (
            "{",
            serde_json::from_str::<serde_json::Value>("{").unwrap_err().to_string(),
        ),
        ("{}", "ipPath 'data.ips' not found in response".into()),
        (r#"{"data":{"ips":1}}"#, "ipPath did not point to array/string".into()),
        (r#"{"data":{"ips":"192.0.2.1"}}"#, "delimiter must not be empty".into()),
    ] {
        assert!(matches!(response.parse(body), Err(DnsError::Parse(message)) if message == expected));
    }
}

#[tokio::test]
async fn complete_query_rejects_merged_overflow_and_overflow_before_family_filter() {
    struct ManyAddresses;
    #[async_trait::async_trait]
    impl HttpDnsFetcher for ManyAddresses {
        async fn get(&self, url: &str) -> Result<String, DnsError> {
            let ips: Vec<String> = if url.ends_with("=A") {
                (1..=128).map(|i| format!("192.0.2.{i}")).collect()
            } else {
                (1..=256)
                    .map(|i| format!("2001:db8::{i:x}"))
                    .chain(std::iter::once("192.0.2.1".into()))
                    .collect()
            };
            Ok(serde_json::json!({"ips":ips}).to_string())
        }
    }
    let combined = generic("https://dns.test/{domain}", HttpDnsResponseFormat::default());
    assert!(resolve_http_dns_with(
        &ManyAddresses,
        &combined,
        "backend.test",
        DnsQueryPolicy::Family(DnsLookupFamily::Ipv4Only)
    )
    .await
    .is_err());
    struct MergedOverflow;
    #[async_trait::async_trait]
    impl HttpDnsFetcher for MergedOverflow {
        async fn get(&self, url: &str) -> Result<String, DnsError> {
            let ips: Vec<String> = if url.ends_with("=A") {
                (1..=128).map(|i| format!("192.0.2.{i}")).collect()
            } else {
                (1..=129).map(|i| format!("2001:db8::{i:x}")).collect()
            };
            Ok(serde_json::json!({"ips":ips}).to_string())
        }
    }
    let typed = generic(
        "https://dns.test/{domain}?type={recordType}",
        HttpDnsResponseFormat::default(),
    );
    assert!(resolve_http_dns_with(
        &MergedOverflow,
        &typed,
        "backend.test",
        DnsQueryPolicy::Family(DnsLookupFamily::All)
    )
    .await
    .is_err());
}

#[test]
fn ttl_path_accepts_integer_numbers_and_strings_and_ignores_anything_else() {
    let format = HttpDnsResponseFormat {
        ttl_path: Some("data.ttl".into()),
        ..HttpDnsResponseFormat::default()
    };
    let response = PreparedResponse::from_format(&format);
    let secs = |n| Some(std::time::Duration::from_secs(n));
    for (body, expected) in [
        (r#"{"ips":["192.0.2.1"],"data":{"ttl":60}}"#, secs(60)),
        (r#"{"ips":["192.0.2.1"],"data":{"ttl":" 15 "}}"#, secs(15)),
        (r#"{"ips":["192.0.2.1"],"data":{"ttl":-5}}"#, None),
        (r#"{"ips":["192.0.2.1"],"data":{"ttl":1.5}}"#, None),
        (r#"{"ips":["192.0.2.1"],"data":{"ttl":"soon"}}"#, None),
        (r#"{"ips":["192.0.2.1"]}"#, None),
    ] {
        let answer = response.parse(body).unwrap();
        assert_eq!(answer.addrs, vec!["192.0.2.1".parse::<IpAddr>().unwrap()], "{body}");
        assert_eq!(answer.ttl, expected, "{body}");
        assert_eq!(response.parse_complete(body).unwrap().ttl, expected, "{body}");
    }
    let without_path = PreparedResponse::from_format(&HttpDnsResponseFormat::default());
    assert_eq!(
        without_path.parse(r#"{"ips":["192.0.2.1"],"ttl":60}"#).unwrap().ttl,
        None
    );
}

#[tokio::test]
async fn typed_answer_ttl_is_the_minimum_over_families_with_addresses() {
    let format = HttpDnsResponseFormat {
        ttl_path: Some("ttl".into()),
        ..HttpDnsResponseFormat::default()
    };
    let cfg = generic("https://dns.test/?host={domain}&type={recordType}", format);
    let all = DnsQueryPolicy::Family(DnsLookupFamily::All);
    let secs = |n| Some(std::time::Duration::from_secs(n));
    for (v4, v6, expected) in [
        (
            r#"{"ips":["192.0.2.1"],"ttl":60}"#,
            r#"{"ips":["2001:db8::1"],"ttl":20}"#,
            secs(20),
        ),
        // An empty family does not shorten the answer lifetime.
        (r#"{"ips":["192.0.2.1"],"ttl":60}"#, r#"{"ips":[],"ttl":1}"#, secs(60)),
        // A family without a TTL does not erase the other family's TTL.
        (
            r#"{"ips":["192.0.2.1"],"ttl":60}"#,
            r#"{"ips":["2001:db8::1"]}"#,
            secs(60),
        ),
        (r#"{"ips":["192.0.2.1"]}"#, r#"{"ips":["2001:db8::1"]}"#, None),
    ] {
        let answer = resolve_http_dns_with(&TypedFetcher { v4, v6 }, &cfg, "backend.test", all)
            .await
            .unwrap();
        assert_eq!(answer.ttl, expected, "{v4} / {v6}");
    }
}
