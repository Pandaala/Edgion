use super::*;

#[test]
fn response_cache_literal_authority_is_bounded_and_contextual_templates_never_resolve_variables() {
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::upstream_authority::{CompiledUpstreamAuthority, UpstreamAuthorityConfig};
    let make = |template: &str| {
        CompiledUpstreamAuthority::compile(&UpstreamAuthorityConfig {
            pattern: "*.sandbox.internal".into(),
            template: template.into(),
            health_check_host: None,
        })
        .unwrap()
    };
    // Operator admission requires a variable. Literal handling is defensive
    // for an already compiled internal artifact, not an operator profile.
    let literal = CompiledUpstreamAuthority {
        label_template: edgion_resources::resources::common::request_template::CompiledTemplate::compile("tenant")
            .unwrap(),
        suffix: ".sandbox.internal".into(),
        health_check_host: None,
    };
    assert!(authority_is_eligible(&literal));
    let session = MockPluginSession::new();
    let value = crate::backends::policy::backend_traffic::authority::render_authority(&literal, &session).unwrap();
    assert_ne!(authority_stamp(None), authority_stamp(Some(&value)));
    assert_ne!(
        authority_stamp(Some("other.sandbox.internal")),
        authority_stamp(Some(&value))
    );
    for template in [
        "${ctx:tenant}.sandbox.internal",
        "${remoteIp}.sandbox.internal",
        "${matchedHttpRoute:name}.sandbox.internal",
    ] {
        let contextual = make(template);
        let (eligible, allocations) =
            crate::test_allocation_counter::count_allocations(|| authority_is_eligible(&contextual));
        assert!(!eligible);
        assert_eq!(allocations, 0);
    }
    let mut oversized = literal;
    oversized.suffix = "a".repeat(254);
    assert!(!authority_is_eligible(&oversized));
    assert!(authority_stamp(Some(&"a".repeat(317))).is_none());
}

fn digest(headers: &HeaderMap, testing: bool) -> Option<[u8; 32]> {
    let mut key = Builder::new();
    key.headers(headers, &RequestTraceId::default(), testing)?;
    Some(key.finish())
}

#[test]
fn response_cache_header_order_is_normalized_and_repeated_value_order_is_material() {
    let mut first = HeaderMap::new();
    first.insert("x-b", "second".parse().unwrap());
    first.append("x-a", "one".parse().unwrap());
    first.append("x-a", "two".parse().unwrap());
    let mut second = HeaderMap::new();
    second.append("x-a", "one".parse().unwrap());
    second.append("x-a", "two".parse().unwrap());
    second.insert("x-b", "second".parse().unwrap());
    assert_eq!(digest(&first, false), digest(&second, false));
    second.remove("x-a");
    second.append("x-a", "two".parse().unwrap());
    second.append("x-a", "one".parse().unwrap());
    assert_ne!(digest(&first, false), digest(&second, false));
    let mut framed = Builder::new();
    framed.frame(b"ab").unwrap();
    framed.frame(b"c").unwrap();
    let mut other = Builder::new();
    other.frame(b"a").unwrap();
    other.frame(b"bc").unwrap();
    assert_ne!(framed.finish(), other.finish());
}

#[test]
fn response_cache_connection_and_testing_exclusions_remain_bounded() {
    let mut headers = HeaderMap::new();
    let empty = digest(&headers, false).unwrap();
    headers.insert("connection", " X-Hop, keep-alive ".parse().unwrap());
    headers.insert("x-hop", "private-transport".parse().unwrap());
    headers.insert("keep-alive", "timeout=5".parse().unwrap());
    assert_eq!(digest(&headers, false), Some(empty));
    headers.insert("access_log", "test".parse().unwrap());
    assert_eq!(digest(&headers, true), Some(empty));
    assert_ne!(digest(&headers, false), Some(empty));
    headers.append("connection", "other".parse().unwrap());
    assert!(digest(&headers, false).is_none());
    headers.remove("connection");
    headers.insert("connection", ",".parse().unwrap());
    assert!(digest(&headers, false).is_none());
    headers.insert(
        "connection",
        (0..17)
            .map(|i| format!("x-{i}"))
            .collect::<Vec<_>>()
            .join(",")
            .parse()
            .unwrap(),
    );
    assert!(digest(&headers, false).is_none());
    headers.insert("connection", "x".repeat(513).parse().unwrap());
    assert!(digest(&headers, false).is_none());
    for invalid in ["", " x hop ", "x-hop,", "x-hop,,other", "x-hop;other"] {
        headers.insert("connection", invalid.parse().unwrap());
        assert!(digest(&headers, false).is_none());
    }
    headers.clear();
    headers.insert("connection", "X-Hop, x-hop".parse().unwrap());
    headers.insert("x-hop", "hidden".parse().unwrap());
    assert_eq!(digest(&headers, false), Some(empty));
}

#[test]
fn response_cache_key_material_and_header_count_refuse_without_allocating() {
    let mut builder = Builder::new();
    let bytes = vec![b'a'; MATERIAL_LIMIT - 8];
    builder.frame(&bytes).unwrap();
    assert!(builder.frame(b"").is_none());
    let mut headers = HeaderMap::new();
    for index in 0..65 {
        headers.insert(
            HeaderName::from_bytes(format!("x-{index}").as_bytes()).unwrap(),
            "v".parse().unwrap(),
        );
    }
    let (result, calls) = crate::test_allocation_counter::count_allocations(|| digest(&headers, false));
    assert!(result.is_none());
    assert_eq!(calls, 0);
    let mut admitted = HeaderMap::new();
    admitted.insert("x-effective", "value".parse().unwrap());
    let (_, calls) = crate::test_allocation_counter::count_allocations(|| digest(&admitted, false).unwrap());
    assert_eq!(
        calls, 0,
        "normalization borrows headers and sorts a bounded stack index"
    );
}

#[test]
fn response_cache_vary_normalization_is_case_independent_bounded_and_exclusions_cannot_return_via_variation() {
    let mut certificate = HeaderCertificate::default();
    certificate.add("x-public").unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("vary", " X-Public, x-public ".parse().unwrap());
    let vary = VaryCertificate::parse(&headers).unwrap();
    assert!(certificate.permits(&vary));
    headers.insert("vary", "x-private".parse().unwrap());
    assert!(!certificate.permits(&VaryCertificate::parse(&headers).unwrap()));
    headers.insert("vary", "*".parse().unwrap());
    assert!(VaryCertificate::parse(&headers).is_none());
    headers.insert(
        "vary",
        (0..17)
            .map(|i| format!("x-{i}"))
            .collect::<Vec<_>>()
            .join(",")
            .parse()
            .unwrap(),
    );
    assert!(VaryCertificate::parse(&headers).is_none());
    headers.clear();
    headers.insert("connection", "x-hop".parse().unwrap());
    let trace = RequestTraceId::default();
    let exclusions = Exclusions::parse(&headers, &trace, false).unwrap();
    for name in ["x-hop", "X-HOP", "host", "content-length"] {
        assert!(exclusions.contains(name));
    }
    for name in ["x-public", "x-other", "access_log"] {
        assert!(!exclusions.contains(name));
    }
    assert!(Exclusions::parse(&headers, &trace, true)
        .unwrap()
        .contains("access_log"));
}

#[test]
fn bounded_connection_index_borrows_all_slots_and_deduplicates_without_allocating() {
    let trace = RequestTraceId::default();
    let mut headers = HeaderMap::new();
    let empty = digest(&headers, false).unwrap();
    let names: Vec<_> = (0..16).map(|index| format!("x-hop-{index}")).collect();
    headers.insert("connection", names.join(",").parse().unwrap());
    for name in &names {
        headers.insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            "hidden".parse().unwrap(),
        );
    }
    let ((key, representable), calls) = crate::test_allocation_counter::count_allocations(|| {
        (
            digest(&headers, false),
            !Exclusions::parse(&headers, &trace, false).unwrap().contains("X-HOP-15"),
        )
    });
    assert_eq!(key, Some(empty));
    assert!(!representable);
    assert_eq!(calls, 0);

    headers.insert("connection", " X-Hop-0, x-hop-0, X-HOP-0 ".parse().unwrap());
    let (exclusions, calls) =
        crate::test_allocation_counter::count_allocations(|| Exclusions::parse(&headers, &trace, false).unwrap());
    assert_eq!(calls, 0);
    assert_eq!(exclusions.connection_count, 1);
    assert!(exclusions.contains("x-hop-0"));
    assert!(!exclusions.contains("x-hop-1"));

    headers.insert("connection", ["x-hop-0"; 17].join(",").parse().unwrap());
    let (result, calls) = crate::test_allocation_counter::count_allocations(|| digest(&headers, false));
    assert!(result.is_none(), "the grammar counts duplicates before deduplication");
    assert_eq!(calls, 0);
}

#[test]
fn header_name_digest_preserves_bytewise_framing_without_allocations() {
    fn reference(name: &str) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(b"edgion response cache header name\0");
        for byte in name.bytes() {
            digest.update([byte.to_ascii_lowercase()]);
        }
        digest.finalize().into()
    }
    let mut names = Vec::new();
    for length in (0..=257).chain([511, 512, 513, 4096, 16384]) {
        let alphabet = b"AbCd-X_09!%";
        names.push(
            (0..length)
                .map(|index| char::from(alphabet[index % alphabet.len()]))
                .collect::<String>(),
        );
    }
    names.extend(["vary", "VARY", "", "X-Café", "x-π", "x-\0\n\t"].map(str::to_owned));
    for prefix in [62, 63, 64, 126, 127, 128] {
        for suffix in ["é", "π", "€"] {
            names.push(format!("{}{}Z", "A".repeat(prefix), suffix));
        }
    }
    for name in names {
        let expected = reference(&name);
        let (actual, allocations) = crate::test_allocation_counter::count_allocations(|| header_digest(&name));
        assert_eq!(
            actual,
            expected,
            "digest must preserve all bytes at length {}",
            name.len()
        );
        assert_eq!(allocations, 0);
    }
    assert_eq!(header_digest("X-Public"), header_digest("x-public"));
    assert_ne!(header_digest("x-public"), header_digest("x-private"));
}
