use super::*;

#[test]
fn endpoint_literal_projection_preserves_normalized_and_opaque_hosts() {
    for scheme in ["http", "https", "ws", "ftp", "custom"] {
        for host in [
            "127.0.0.1",
            "127.1",
            "2130706433",
            "0x7f000001",
            "0177.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "0.0.0.0",
            "[::]",
            "[::1]",
            "[::ffff:127.0.0.1]",
            "[::ffff:169.254.169.254]",
            "[fe80::1]",
            "[2001:db8::1]",
            "localhost",
            "example.test",
            "é.test",
        ] {
            let url = reqwest::Url::parse(&format!("{scheme}://{host}/d?host=example.test")).unwrap();
            let serialized = url.as_str().to_owned();
            let previous = url
                .host_str()
                .and_then(|host| super::super::strip_brackets(host).parse::<IpAddr>().ok());
            assert_eq!(endpoint_literal_ip(&url), previous, "{serialized}");
            assert_eq!(url.as_str(), serialized);
        }
    }
    for input in [
        "file:///tmp/dns",
        "file://localhost/tmp/dns",
        "data:text/plain,dns",
        "mailto:user@example.test",
    ] {
        let url = reqwest::Url::parse(input).unwrap();
        assert_eq!(endpoint_literal_ip(&url), None, "{input}");
    }
    assert_eq!(
        endpoint_literal_ip(&reqwest::Url::parse("custom://127.0.0.1/d").unwrap()),
        Some("127.0.0.1".parse().unwrap())
    );
    assert_eq!(
        endpoint_literal_ip(&reqwest::Url::parse("custom://127.1/d").unwrap()),
        None
    );
}
