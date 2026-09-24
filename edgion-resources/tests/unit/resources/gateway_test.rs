use super::*;

#[test]
fn gateway_listener_secrets_redacted_under_guard() {
    use crate::redact::AdminRedactGuard;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    let mut data = BTreeMap::new();
    data.insert("tls.key".to_string(), ByteString(b"LISTENER-PRIVATE-KEY".to_vec()));
    let cfg = GatewayTLSConfig {
        mode: None,
        certificate_refs: None,
        options: None,
        resolved_certificate_refs: Some(vec![ResolvedServingCertificateReference {
            index: 0,
            source: ServingCertificateSourceIdentity {
                group: String::new(),
                kind: "Secret".to_string(),
                namespace: "default".to_string(),
                name: "server".to_string(),
            },
            secret: Some(Secret {
                data: Some(data),
                ..Default::default()
            }),
            error: None,
        }]),
        frontend_validation: None,
        resolved_frontend_ca_refs: None,
        frontend_matcher_eligibility: None,
    };

    // Under the admin guard, the resolved listener serving cert+key is redacted.
    let redacted = {
        let _g = AdminRedactGuard::new();
        serde_json::to_value(&cfg).unwrap()
    };
    assert_eq!(redacted["resolvedCertificateRefs"], "[redacted]");
    assert!(
        !redacted.to_string().contains("LISTENER-PRIVATE-KEY"),
        "listener private key leaked under guard"
    );

    // Transport (no guard) must still emit the real Secret objects.
    let plain = serde_json::to_value(&cfg).unwrap();
    assert!(
        plain["resolvedCertificateRefs"].is_array(),
        "real listener secrets must serialize for transport"
    );
    let roundtrip: GatewayTLSConfig = serde_json::from_value(plain).unwrap();
    let outcomes = roundtrip.resolved_certificate_refs.unwrap();
    let key = &outcomes[0].secret.as_ref().unwrap().data.as_ref().unwrap()["tls.key"].0;
    assert_eq!(key, b"LISTENER-PRIVATE-KEY");
}

#[test]
fn test_gateway_serialization() {
    let gateway = Gateway {
        metadata: kube::core::ObjectMeta {
            name: Some("test-gateway".to_string()),
            namespace: Some("default".to_string()),
            ..Default::default()
        },
        spec: GatewaySpec {
            current_status: None,
            gateway_class_name: "test-class".to_string(),
            listeners: Some(vec![Listener {
                name: "http".to_string(),
                hostname: None,
                port: 80,
                protocol: "HTTP".to_string(),
                tls: None,
                allowed_routes: None,
            }]),
            addresses: None,
            tls: None,
            allowed_listeners: None,
            infrastructure: None,
            default_scope: None,
            resolved_inbound_proxy_protocol: ResolvedInboundProxyProtocol::Disabled,
            resolved_attachment_proof: None,
        },
        status: None,
    };

    let yaml = serde_yaml::to_string(&gateway).unwrap();
    println!("{}", yaml);
    assert!(yaml.contains("gatewayClassName"));
}

#[test]
fn test_frontend_validation_json_roundtrip() {
    let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: test-fv
  namespace: default
spec:
  gatewayClassName: test-class
  listeners:
    - name: https
      hostname: test.example.com
      port: 443
      protocol: HTTPS
  tls:
    frontend:
      default:
        validation:
          caCertificateRefs:
            - name: client-ca
              kind: Secret
              group: ""
"#;
    let gw: Gateway = serde_yaml::from_str(yaml).unwrap();
    let json = serde_json::to_string_pretty(&gw).unwrap();
    println!("JSON:\n{}", json);
    assert!(
        json.contains("caCertificateRefs"),
        "standard frontend TLS missing in JSON: {}",
        json
    );
}

#[test]
fn test_frontend_validation_deserialization() {
    let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: test-fv
  namespace: default
spec:
  gatewayClassName: test-class
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
  tls:
    frontend:
      default:
        validation:
          caCertificateRefs:
            - name: client-ca
              kind: Secret
              group: ""
"#;
    let gw: Gateway = serde_yaml::from_str(yaml).unwrap();
    let fv = gw.spec.tls.unwrap().frontend.unwrap().default.validation.unwrap();
    let ca_refs = &fv.ca_certificate_refs;
    assert_eq!(ca_refs.len(), 1);
    assert_eq!(ca_refs[0].name, "client-ca");
}

#[test]
fn frontend_validation_mode_accepts_known_values() {
    for (input, expected) in [
        ("AllowValidOnly", FrontendValidationMode::AllowValidOnly),
        ("AllowInsecureFallback", FrontendValidationMode::AllowInsecureFallback),
    ] {
        let yaml = format!("caCertificateRefs:\n  - group: \"\"\n    kind: ConfigMap\n    name: ca\nmode: {input}\n");
        let fv: FrontendTLSValidation = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(fv.mode, Some(expected));
    }
}

#[test]
fn frontend_validation_mode_default_when_absent() {
    let fv: FrontendTLSValidation =
        serde_yaml::from_str("caCertificateRefs:\n  - group: \"\"\n    kind: ConfigMap\n    name: ca\n").unwrap();
    assert!(fv.mode.is_none());
    // Consumers fall back to AllowValidOnly via Default when None.
    assert_eq!(
        FrontendValidationMode::default(),
        FrontendValidationMode::AllowValidOnly
    );
}

#[test]
fn frontend_validation_mode_rejects_typo() {
    // Previous behavior silently mapped unknown strings to the strict default.
    // After typing the field, unknown values must fail deserialization so the
    // controller surfaces the error instead of silently overriding intent.
    let err = serde_yaml::from_str::<FrontendTLSValidation>(
        "caCertificateRefs:\n  - group: \"\"\n    kind: ConfigMap\n    name: ca\nmode: AllowFoo\n",
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("AllowFoo") || msg.contains("variant"),
        "expected variant error, got: {msg}"
    );
}

#[test]
fn frontend_validation_requires_standard_reference_shape_and_nonempty_set() {
    for yaml in [
        "caCertificateRefs: []\n",
        "caCertificateRefs:\n  - kind: ConfigMap\n    name: ca\n",
        "caCertificateRefs:\n  - group: \"\"\n    name: ca\n",
        "caCertificateRefs:\n  - group: \"\"\n    kind: ConfigMap\n    name: \"\"\n",
    ] {
        assert!(
            serde_yaml::from_str::<FrontendTLSValidation>(yaml).is_err(),
            "accepted: {yaml}"
        );
    }
}

#[test]
fn frontend_ca_reference_limit_matches_v16_schema() {
    let reference = serde_json::json!({"group": "", "kind": "ConfigMap", "name": "ca"});
    for count in [9, 16] {
        let value = serde_json::json!({"caCertificateRefs": vec![reference.clone(); count]});
        let parsed: FrontendTLSValidation = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.ca_certificate_refs.len(), count);
    }
    let too_many = serde_json::json!({"caCertificateRefs": vec![reference; 17]});
    assert!(serde_json::from_value::<FrontendTLSValidation>(too_many).is_err());

    let schema = serde_json::to_value(schemars::schema_for!(FrontendTLSValidation)).unwrap();
    assert_eq!(
        schema.pointer("/properties/caCertificateRefs/maxItems"),
        Some(&serde_json::json!(16))
    );
}

#[test]
fn frontend_wire_shape_requires_default_and_per_port_tls_but_allows_empty_policy() {
    let missing_default = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata: { name: test }
spec:
  gatewayClassName: test
  tls:
    frontend:
      perPort: []
"#;
    assert!(serde_yaml::from_str::<Gateway>(missing_default).is_err());

    let missing_per_port_tls = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata: { name: test }
spec:
  gatewayClassName: test
  tls:
    frontend:
      default: {}
      perPort:
      - port: 443
"#;
    assert!(serde_yaml::from_str::<Gateway>(missing_per_port_tls).is_err());

    let empty_policies = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata: { name: test }
spec:
  gatewayClassName: test
  tls:
    frontend:
      default: {}
      perPort:
      - port: 443
        tls: {}
"#;
    let gateway = serde_yaml::from_str::<Gateway>(empty_policies).unwrap();
    let frontend = gateway.spec.tls.unwrap().frontend.unwrap();
    assert!(frontend.default.validation.is_none());
    assert!(frontend.per_port.unwrap()[0].tls.validation.is_none());
}

#[test]
fn beta_conversion_preserves_gateway_fields_and_known_unsupported_fields() {
    let source = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "Gateway",
        "metadata": {"name": "edge", "namespace": "demo", "resourceVersion": "17"},
        "spec": {
            "gatewayClassName": "edgion",
            "addresses": [{"type": "IPAddress", "value": "192.0.2.1"}],
            "listeners": [{"name": "https", "port": 443, "protocol": "HTTPS",
                "hostname": "example.com",
                "tls": {"mode": "Terminate", "certificateRefs": [{"name": "server"}]},
                "allowedRoutes": {"namespaces": {"from": "Selector", "selector": {"matchLabels": {"team": "web"}}},
                                  "kinds": [{"kind": "HTTPRoute"}]}}]
        },
        "status": {"addresses": [{"type": "IPAddress", "value": "192.0.2.1"}],
                   "listeners": [{"name": "https", "supportedKinds": [{"kind": "HTTPRoute"}],
                                  "attachedRoutes": 2, "conditions": []}]}
    });
    let beta: GatewayBeta = serde_json::from_value(source).unwrap();
    let gateway: Gateway = beta.into();
    assert_eq!(gateway.metadata.resource_version.as_deref(), Some("17"));
    assert_eq!(
        gateway.spec.addresses.as_ref().unwrap()[0].value.as_deref(),
        Some("192.0.2.1")
    );
    let listener = &gateway.spec.listeners.as_ref().unwrap()[0];
    assert_eq!(
        listener.tls.as_ref().unwrap().certificate_refs.as_ref().unwrap()[0].name,
        "server"
    );
    assert_eq!(
        listener
            .allowed_routes
            .as_ref()
            .unwrap()
            .namespaces
            .as_ref()
            .unwrap()
            .from
            .as_deref(),
        Some("Selector")
    );
    assert_eq!(gateway.status.unwrap().listeners.unwrap()[0].attached_routes, 2);

    let mut unsupported = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "Gateway",
        "metadata": {"name": "edge", "namespace": "demo"},
        "spec": {"gatewayClassName": "edgion"}
    });
    for field in ["allowedListeners", "infrastructure", "defaultScope"] {
        unsupported["spec"][field] = serde_json::json!({"known": true});
        let beta: GatewayBeta = serde_json::from_value(unsupported.clone()).unwrap();
        let gateway: Gateway = beta.into();
        assert!(match field {
            "allowedListeners" => gateway.spec.allowed_listeners.is_some(),
            "infrastructure" => gateway.spec.infrastructure.is_some(),
            _ => gateway.spec.default_scope.is_some(),
        });
        unsupported["spec"].as_object_mut().unwrap().remove(field);
    }
    unsupported["spec"]["futureField"] = serde_json::json!(true);
    assert!(serde_json::from_value::<GatewayBeta>(unsupported).is_ok());
}

#[test]
fn unsupported_diagnostic_fields_do_not_enter_the_operator_schema() {
    let schema = serde_json::to_value(schemars::schema_for!(GatewaySpec))
        .unwrap()
        .to_string();
    for field in ["allowedListeners", "infrastructure", "defaultScope"] {
        assert!(!schema.contains(field));
    }
}

#[test]
fn listener_projection_fields_are_schema_hidden_and_ca_material_is_redacted() {
    use crate::redact::AdminRedactGuard;
    use k8s_openapi::ByteString;
    use schemars::schema_for;
    use std::collections::BTreeMap;

    let schema = serde_json::to_value(schema_for!(GatewayTLSConfig)).unwrap();
    let schema_json = schema.to_string();
    assert!(!schema_json.contains("frontendValidation"));
    assert!(!schema_json.contains("resolvedFrontendCaRefs"));

    let mut data = BTreeMap::new();
    data.insert("ca.crt".to_string(), ByteString(b"PRIVATE-CLIENT-CA".to_vec()));
    let cfg = GatewayTLSConfig {
        mode: None,
        certificate_refs: None,
        options: None,
        resolved_certificate_refs: None,
        frontend_validation: None,
        resolved_frontend_ca_refs: Some(vec![ResolvedFrontendCAReference {
            index: 0,
            source: FrontendCASourceIdentity {
                kind: FrontendCASourceKind::Secret,
                namespace: "default".to_string(),
                name: "client-ca".to_string(),
            },
            ca_certificate: Some(data.remove("ca.crt").unwrap()),
            error: None,
        }]),
        frontend_matcher_eligibility: None,
    };
    let redacted = {
        let _guard = AdminRedactGuard::new();
        serde_json::to_value(&cfg).unwrap()
    };
    assert_eq!(redacted["resolvedFrontendCaRefs"], "[redacted]");
    assert!(!redacted.to_string().contains("PRIVATE-CLIENT-CA"));
}
