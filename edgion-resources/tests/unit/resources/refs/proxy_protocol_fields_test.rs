use super::*;
use serde_json::json;

fn gateway(annotation: Option<&str>) -> Gateway {
    let mut owner: Gateway = serde_json::from_value(json!({
        "metadata":{"name":"gateway"}, "spec":{"gatewayClassName":"public"}
    }))
    .unwrap();
    owner.metadata.annotations =
        annotation.map(|value| std::collections::BTreeMap::from([(INBOUND_PROXY_PROTOCOL.into(), value.into())]));
    owner
}

#[test]
fn actual_proxy_reference_defaults_and_exact_typed_query_do_not_rewrite_raw_owner() {
    for (owner_ns, target_ns, expected) in [
        (None, None, "default"),
        (Some("apps"), None, "apps"),
        (None, Some("policy"), "policy"),
    ] {
        let annotation =
            json!({"source":"resourceRef","resourceRef":{"name":"trust","namespace":target_ns}}).to_string();
        let mut owner = gateway(Some(&annotation));
        owner.metadata.namespace = owner_ns.map(str::to_owned);
        let field = GatewayProxyProtocolField::parse(&owner).unwrap().unwrap();
        let query = field.query().unwrap().unwrap();
        assert_eq!(
            query.source().scope(),
            ResourceScope::Namespaced(owner_ns.unwrap_or("default"))
        );
        assert_eq!(query.source().group_kind().group(), "gateway.networking.k8s.io");
        assert_eq!(query.source().group_kind().kind(), "Gateway");
        assert_eq!(query.target().scope(), ResourceScope::Namespaced(expected));
        assert_eq!(query.target().group_kind().group(), "edgion.io");
        assert_eq!(query.target().group_kind().kind(), "EdgionConfigData");
        assert_eq!(query.target().name(), "trust");
        assert_eq!(owner.metadata.namespace.as_deref(), owner_ns);
    }
}

#[test]
fn proxy_reference_checks_actual_owner_but_absence_and_inline_are_independent() {
    for (namespace, name) in [
        (Some(""), Some("gateway")),
        (Some("UPPER"), Some("gateway")),
        (None, None),
        (None, Some("bad/name")),
    ] {
        let mut owner = gateway(Some(
            r#"{"source":"resourceRef","resourceRef":{"name":"trust","namespace":"policy"}}"#,
        ));
        owner.metadata.namespace = namespace.map(str::to_owned);
        owner.metadata.name = name.map(str::to_owned);
        assert_eq!(
            GatewayProxyProtocolField::parse(&owner).unwrap().unwrap().query(),
            Err(ProxyProtocolConfigErrorCode::InvalidReference)
        );
        owner.metadata.annotations = None;
        assert!(GatewayProxyProtocolField::parse(&owner).unwrap().is_none());
        owner.metadata.annotations = Some(std::collections::BTreeMap::from([(
            INBOUND_PROXY_PROTOCOL.into(),
            r#"{"source":"inline","config":{"mode":"required"}}"#.into(),
        )]));
        let field = GatewayProxyProtocolField::parse(&owner).unwrap().unwrap();
        assert!(field.query().unwrap().is_none());
        assert!(field.policy().unwrap().canonicalize().is_ok());
    }
}

#[test]
fn actual_annotation_uses_canonical_parse_errors_before_any_query() {
    for (value, code) in [
        ("", ProxyProtocolConfigErrorCode::MalformedJson),
        (
            r#"{"source":"resourceRef","resourceRef":{"name":"trust","namespace":""}}"#,
            ProxyProtocolConfigErrorCode::InvalidReference,
        ),
        (
            r#"{"source":"resourceRef","resourceRef":{"name":"bad/name"}}"#,
            ProxyProtocolConfigErrorCode::InvalidReference,
        ),
        (
            r#"{"source":"resourceRef","resourceRef":{"name":"trust","refDenied":false}}"#,
            ProxyProtocolConfigErrorCode::InvalidShape,
        ),
    ] {
        assert!(matches!(GatewayProxyProtocolField::parse(&gateway(Some(value))),Err(actual) if actual==code));
    }
}
