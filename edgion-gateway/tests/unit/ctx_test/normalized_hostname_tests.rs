use super::NormalizedHostname;

#[test]
fn client_certificate_debug_omits_identity() {
    let info = super::ClientCertInfo {
        subject: "private-subject".into(),
        sans: vec!["private-san".into()],
        cn: Some("private-cn".into()),
        fingerprint: "public-fingerprint".into(),
    };
    for debug in [format!("{info:?}"), format!("{info:#?}"), format!("{:?}", Some(&info))] {
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("private"));
    }
    assert_eq!(info.subject, "private-subject");
}

#[test]
fn normalization_strips_only_unambiguous_ports() {
    for (raw, expected) in [
        ("Example.COM:8443", "example.com"),
        ("127.0.0.1:8080", "127.0.0.1"),
        ("[2001:db8::1]:443", "2001:db8::1"),
        ("2001:db8::1", "2001:db8::1"),
        ("", ""),
        ("   ", ""),
        (":", ""),
        (":443", ""),
        ("Example.COM", "example.com"),
        ("Example.COM:", "example.com"),
        ("Example.COM:bad", "example.com:bad"),
        ("Example.COM:+443", "example.com:+443"),
        ("Example.COM:443x", "example.com:443x"),
        ("Example.COM::443", "example.com::443"),
        ("::", "::"),
        ("2001:DB8::1:443", "2001:db8::1:443"),
        ("[2001:DB8::1]suffix", "2001:db8::1"),
        ("[2001:DB8::1", "[2001:db8::1"),
        ("[]:443", ""),
        (" Example.COM.:8443 ", "example.com"),
        ("Example.COM..", "example.com."),
        ("éXAMPLE.COM:443", "éxample.com"),
    ] {
        assert_eq!(NormalizedHostname::new(raw).as_str(), expected, "raw: {raw:?}");
    }
}
