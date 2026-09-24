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
    assert_eq!(NormalizedHostname::new("Example.COM:8443").as_str(), "example.com");
    assert_eq!(NormalizedHostname::new("127.0.0.1:8080").as_str(), "127.0.0.1");
    assert_eq!(NormalizedHostname::new("[2001:db8::1]:443").as_str(), "2001:db8::1");
    assert_eq!(NormalizedHostname::new("2001:db8::1").as_str(), "2001:db8::1");
}
