use super::*;
use std::collections::BTreeMap;

#[test]
fn dns_grammar_and_actual_owner_sources_remain_distinct() {
    for (value, expected) in [
        (" ns / Name_With.Underscore ", Some(("ns", "Name_With.Underscore"))),
        ("ns/name", Some(("ns", "name"))),
        ("name", None),
        (" /name", None),
        ("ns/ ", None),
        ("ns/a/b", None),
    ] {
        assert_eq!(DnsLinkSysKey::parse(value).map(|k| (k.namespace(), k.name())), expected);
    }
    let mut service = Service {
        metadata: ObjectMeta {
            name: Some("source".into()),
            annotations: Some(BTreeMap::from([(DNS_RESOLVER_REF.into(), "dns/name".into())])),
            ..Default::default()
        },
        ..Default::default()
    };
    let query = DnsResolverOwner::Service(&service).query().unwrap().unwrap();
    assert_eq!(query.source().group_kind().group(), "");
    assert_eq!(query.source().group_kind().kind(), "Service");
    assert_eq!(query.source().scope().namespace(), Some("default"));
    let gateway:Gateway=serde_json::from_value(serde_json::json!({"metadata":{"name":"source","annotations":{DNS_RESOLVER_REF:"dns/name"}},"spec":{"gatewayClassName":"test","listeners":[]}})).unwrap();
    let query = DnsResolverOwner::Gateway(&gateway).query().unwrap().unwrap();
    assert_eq!(query.source().group_kind().group(), "gateway.networking.k8s.io");
    assert_eq!(query.source().group_kind().kind(), "Gateway");
    service.metadata.namespace = Some("".into());
    assert_eq!(
        DnsResolverOwner::Service(&service).query(),
        Err(ReferenceError::InvalidNamespace)
    );
    service.metadata.namespace = None;
    service.metadata.name = None;
    assert_eq!(
        DnsResolverOwner::Service(&service).query(),
        Err(ReferenceError::InvalidName)
    );
}
