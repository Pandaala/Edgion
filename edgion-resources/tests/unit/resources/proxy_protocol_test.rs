use schemars::schema_for;
use serde_json::json;

use super::*;
use crate::resources::edgion_config_data::{
    edgion_config_data_spec_schema, ConfigEntry, EdgionConfigData, EdgionConfigDataSpec,
};
use crate::resources::gateway::GatewaySpec;

fn inline_json(config: serde_json::Value) -> String {
    json!({"source": "inline", "config": config}).to_string()
}

#[test]
fn proxy_protocol_annotation_enforces_strict_top_level_union() {
    for value in [
        json!({"config": {"mode": "required"}}),
        json!({"source": "future", "config": {"mode": "required"}}),
        json!({
            "source": "inline",
            "config": {"mode": "required"},
            "resourceRef": {"name": "policy"}
        }),
        json!({
            "source": "resourceRef",
            "resourceRef": {"name": "policy"},
            "config": {"mode": "required"}
        }),
    ] {
        assert_eq!(
            InboundProxyProtocolAnnotation::parse(&value.to_string()),
            Err(ProxyProtocolConfigErrorCode::InvalidShape),
            "unexpectedly accepted {value}"
        );
    }
}

#[test]
fn proxy_protocol_annotation_rejects_unknown_reference_transport_fields() {
    for resource_ref in [
        json!({"name": "policy", "group": "edgion.io"}),
        json!({"name": "policy", "kind": "EdgionConfigData"}),
        json!({"name": "policy", "refDenied": {"reason": "denied"}}),
    ] {
        let value = json!({"source": "resourceRef", "resourceRef": resource_ref});
        assert_eq!(
            InboundProxyProtocolAnnotation::parse(&value.to_string()),
            Err(ProxyProtocolConfigErrorCode::InvalidShape)
        );
    }
}

#[test]
fn proxy_protocol_annotation_distinguishes_malformed_and_oversized_json() {
    assert_eq!(
        InboundProxyProtocolAnnotation::parse("{"),
        Err(ProxyProtocolConfigErrorCode::MalformedJson)
    );
    let oversized = "x".repeat(MAX_INBOUND_PROXY_PROTOCOL_ANNOTATION_BYTES + 1);
    assert_eq!(
        InboundProxyProtocolAnnotation::parse(&oversized),
        Err(ProxyProtocolConfigErrorCode::AnnotationTooLarge)
    );
}

#[test]
fn proxy_protocol_listener_selector_is_bounded_unique_and_valid() {
    for listeners in [json!([]), json!(["http", "http"]), json!(["Uppercase"]), json!([""])] {
        let value = json!({
            "source": "inline",
            "listeners": listeners,
            "config": {"mode": "required"}
        });
        assert_eq!(
            InboundProxyProtocolAnnotation::parse(&value.to_string()),
            Err(ProxyProtocolConfigErrorCode::InvalidListenerSelection)
        );
    }

    let listeners = (0..=MAX_PROXY_PROTOCOL_LISTENERS)
        .map(|index| format!("listener-{index}"))
        .collect::<Vec<_>>();
    let value = json!({
        "source": "inline",
        "listeners": listeners,
        "config": {"mode": "required"}
    });
    assert_eq!(
        InboundProxyProtocolAnnotation::parse(&value.to_string()),
        Err(ProxyProtocolConfigErrorCode::InvalidListenerSelection)
    );
}

#[test]
fn proxy_protocol_reference_names_are_strict_and_bounded() {
    for resource_ref in [
        json!({"name": ""}),
        json!({"name": "UPPER"}),
        json!({"name": "valid", "namespace": "bad.namespace"}),
    ] {
        let value = json!({"source": "resourceRef", "resourceRef": resource_ref});
        assert_eq!(
            InboundProxyProtocolAnnotation::parse(&value.to_string()),
            Err(ProxyProtocolConfigErrorCode::InvalidReference)
        );
    }
}

#[test]
fn proxy_protocol_policy_canonicalizes_bare_and_network_addresses() {
    let policy = ProxyProtocolTrustPolicy::TrustedSources {
        trusted_cidrs: vec![
            " 192.168.1.7 ".to_string(),
            "10.2.3.4/8".to_string(),
            "2001:0db8:0000::1/32".to_string(),
            "2001:db8:1::7".to_string(),
        ],
    };
    assert_eq!(
        policy.canonicalize().unwrap(),
        CanonicalProxyProtocolTrustPolicy::TrustedSources {
            trusted_cidrs: vec![
                "10.0.0.0/8".to_string(),
                "192.168.1.7/32".to_string(),
                "2001:db8:1::7/128".to_string(),
                "2001:db8::/32".to_string(),
            ]
        }
    );
}

#[test]
fn proxy_protocol_policy_rejects_all_invalid_or_unsafe_sets() {
    let invalid_sets = vec![
        vec![],
        vec!["bad".to_string()],
        vec!["0.0.0.0/00".to_string()],
        vec!["::/0".to_string()],
        vec!["10.0.0.1/8".to_string(), "10.0.0.0/8".to_string()],
        vec!["x".repeat(MAX_PROXY_PROTOCOL_CIDR_BYTES + 1)],
        vec![format!("{}10.0.0.1{}", " ".repeat(30), " ".repeat(30))],
    ];
    for trusted_cidrs in invalid_sets {
        let policy = ProxyProtocolTrustPolicy::TrustedSources { trusted_cidrs };
        assert_eq!(policy.canonicalize(), Err(ProxyProtocolConfigErrorCode::InvalidPolicy));
    }

    let policy = ProxyProtocolTrustPolicy::TrustedSources {
        trusted_cidrs: (0..=MAX_PROXY_PROTOCOL_TRUSTED_CIDRS)
            .map(|index| format!("10.{}.{}.1", index / 256, index % 256))
            .collect(),
    };
    assert_eq!(policy.canonicalize(), Err(ProxyProtocolConfigErrorCode::InvalidPolicy));
}

#[test]
fn proxy_protocol_required_mode_has_explicit_wire_form() {
    let annotation = InboundProxyProtocolAnnotation::parse(&inline_json(json!({
        "mode": "required"
    })))
    .unwrap();
    assert_eq!(
        annotation.policy().unwrap().canonicalize().unwrap(),
        CanonicalProxyProtocolTrustPolicy::Required {}
    );
    assert_eq!(
        serde_json::to_value(CanonicalProxyProtocolTrustPolicy::Required {}).unwrap(),
        json!({"mode": "required"})
    );
}

#[test]
fn canonical_proxy_protocol_transport_rejects_noncanonical_or_unsafe_payloads() {
    assert_eq!(
        CanonicalProxyProtocolTrustPolicy::Required {}.validate_transport(),
        Ok(())
    );
    assert_eq!(
        CanonicalProxyProtocolTrustPolicy::TrustedSources {
            trusted_cidrs: vec!["10.0.0.0/8".to_string(), "192.0.2.1/32".to_string()],
        }
        .validate_transport(),
        Ok(())
    );

    let invalid = vec![
        vec![],
        vec!["0.0.0.0/0".to_string()],
        vec!["not-a-cidr".to_string()],
        vec!["x".repeat(MAX_PROXY_PROTOCOL_CIDR_BYTES + 1)],
        vec!["10.0.0.0/8".to_string(), "10.0.0.0/8".to_string()],
        vec!["192.0.2.0/24".to_string(), "10.0.0.0/8".to_string()],
        vec!["10.1.2.3/8".to_string()],
        vec!["192.0.2.1".to_string()],
        vec!["2001:0db8::/32".to_string()],
    ];
    for trusted_cidrs in invalid {
        assert_eq!(
            CanonicalProxyProtocolTrustPolicy::TrustedSources { trusted_cidrs }.validate_transport(),
            Err(ProxyProtocolConfigErrorCode::InvalidPolicy)
        );
    }

    let over_limit = CanonicalProxyProtocolTrustPolicy::TrustedSources {
        trusted_cidrs: (0..=MAX_PROXY_PROTOCOL_TRUSTED_CIDRS)
            .map(|index| format!("10.{}.{}.1/32", index / 256, index % 256))
            .collect(),
    };
    assert_eq!(
        over_limit.validate_transport(),
        Err(ProxyProtocolConfigErrorCode::InvalidPolicy)
    );
}

#[test]
fn proxy_protocol_resolved_state_defaults_disabled_and_round_trips() {
    assert_eq!(
        ResolvedInboundProxyProtocol::default(),
        ResolvedInboundProxyProtocol::Disabled
    );
    assert_eq!(
        serde_json::to_value(ResolvedInboundProxyProtocol::default()).unwrap(),
        json!({"state": "disabled"})
    );

    let spec: GatewaySpec = serde_json::from_value(json!({
        "gatewayClassName": "edgion",
        "listeners": []
    }))
    .unwrap();
    assert_eq!(
        spec.resolved_inbound_proxy_protocol,
        ResolvedInboundProxyProtocol::Disabled
    );
}

#[test]
fn proxy_protocol_resolved_gateway_field_is_not_in_schemars() {
    let schema = schema_for!(GatewaySpec);
    let properties = schema
        .as_value()
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .expect("GatewaySpec schema must contain top-level properties");
    assert!(!properties.contains_key("resolvedInboundProxyProtocol"));
}

#[test]
fn proxy_protocol_config_data_variant_is_strict_without_changing_existing_variants() {
    let strict = json!({
        "data": {
            "type": "ProxyProtocolTrust",
            "config": {"mode": "required", "typo": true}
        }
    });
    let mut strict_spec: EdgionConfigDataSpec =
        serde_json::from_value(strict.clone()).expect("ECD ingest must retain the object for status reporting");
    strict_spec.unknown_fields = crate::schema_check::unknown_key_paths(&strict, edgion_config_data_spec_schema());
    assert_eq!(strict_spec.unknown_fields, ["data.config.typo"]);
    let strict_resource = EdgionConfigData::new("policy", strict_spec);
    assert_eq!(
        strict_resource.validate_pre_parse(),
        ["ProxyProtocolTrust invalid: policy validation failed"]
    );
    assert_eq!(
        InboundProxyProtocolAnnotation::parse(
            &json!({"source": "inline", "config": {"mode": "required", "typo": true}}).to_string()
        ),
        Err(ProxyProtocolConfigErrorCode::InvalidShape)
    );

    let existing = json!({
        "data": {
            "type": "KeyList",
            "config": {"items": [], "typo": true}
        }
    });
    assert!(serde_json::from_value::<EdgionConfigDataSpec>(existing).is_ok());

    let valid: EdgionConfigDataSpec = serde_json::from_value(json!({
        "data": {
            "type": "ProxyProtocolTrust",
            "config": {"mode": "trustedSources", "trustedCidrs": ["10.1.2.3/8"]}
        }
    }))
    .unwrap();
    let ConfigEntry::ProxyProtocolTrust(policy) = valid.data else {
        panic!("expected ProxyProtocolTrust variant")
    };
    assert_eq!(
        policy.canonicalize().unwrap(),
        CanonicalProxyProtocolTrustPolicy::TrustedSources {
            trusted_cidrs: vec!["10.0.0.0/8".to_string()]
        }
    );
}
