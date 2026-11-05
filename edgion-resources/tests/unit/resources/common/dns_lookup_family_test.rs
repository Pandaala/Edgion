use super::*;

#[test]
fn exact_spelling_and_inheritance() {
    assert_eq!(DnsLookupFamily::from_annotation(None), Ok(None));
    for (text, family) in [
        ("ipv4Only", DnsLookupFamily::Ipv4Only),
        ("ipv6Only", DnsLookupFamily::Ipv6Only),
        ("all", DnsLookupFamily::All),
    ] {
        assert_eq!(DnsLookupFamily::from_annotation(Some(text)), Ok(Some(family)));
        assert_eq!(serde_json::to_value(family).unwrap(), text);
        assert_eq!(serde_json::from_value::<DnsLookupFamily>(text.into()).unwrap(), family);
    }
    for text in ["", "auto", "ALL", " ipv4Only", "ipv6Only "] {
        assert!(DnsLookupFamily::from_annotation(Some(text)).is_err());
    }
    assert_eq!(DnsLookupFamily::default(), DnsLookupFamily::Ipv4Only);
}

#[test]
fn mapped_addresses_have_one_canonical_family() {
    let mapped = "::ffff:192.0.2.1".parse().unwrap();
    assert!(DnsLookupFamily::Ipv4Only.allows(mapped));
    assert!(!DnsLookupFamily::Ipv6Only.allows(mapped));
}
